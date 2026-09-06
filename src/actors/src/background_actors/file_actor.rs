use crate::background_actors::cache_actor;
use crate::background_actors::cache_actor::Message::ApplyChanges;
use crate::background_actors::file_actor::Message::{
    ApplyVFS, FileCreated, FileModified, FileRemoved,
};
use analysis::rust_proj::RustProject;
use anyhow::anyhow;
use notify_types::event::{CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode};
use ra_ap_vfs::VfsPath;
use ractor::concurrency::Duration;
use ractor::{Actor, ActorCell, ActorProcessingErr, ActorRef, call};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use tracing::error;
use utils::workspace::{Access, WorkspacePolicy};

pub struct FileActor {}

pub struct Dependency {
    pub(crate) proj: RustProject,
    pub main_dir: PathBuf,
    pub cache_actor: ActorRef<cache_actor::Message>,
}

pub struct FileActorState {
    proj: RustProject,
    inner_file_actor: ActorCell,
    cache_actor: ActorRef<cache_actor::Message>,
}

#[derive(Clone, Debug)]
pub struct ValidPath {
    pub(crate) path: PathBuf,
}

impl ValidPath {
    pub fn new(path: PathBuf, workspace: &WorkspacePolicy) -> Option<Self> {
        if let Some(joe) = path.to_str()
            && !joe.ends_with("~")
            && path.extension().is_some_and(|extension| extension == "rs")
            && workspace.check(&path, Access::Read).is_ok()
        {
            Some(ValidPath { path })
        } else {
            None
        }
    }

    pub fn option(path: Option<PathBuf>, workspace: &WorkspacePolicy) -> Option<Self> {
        path.and_then(|path| Self::new(path, workspace))
    }
}

#[derive(Debug)]
pub enum Message {
    FileCreated(CreateKind, Vec<PathBuf>),
    FileModified(ModifyKind, Vec<PathBuf>),
    FileRemoved(RemoveKind, Vec<PathBuf>),
    ApplyVFS,
}
#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for FileActor {
    type Msg = Message;
    type State = FileActorState;
    type Arguments = Dependency;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Dependency,
    ) -> Result<Self::State, ActorProcessingErr> {
        let fw = FileWatcher;
        let config = FileWatcherConfig {
            directories: vec![dependency.main_dir.clone()],
            files: Vec::new(),
        };

        let (fwactor, fwhandle) = Actor::spawn(None, fw, config).await?;

        let fwrder = Forwarder {
            actor: myself.get_cell(),
        };

        match call!(fwactor, |reply| FileWatcherMessage::Subscribe(
            myself.get_id(),
            Box::new(fwrder),
            reply
        ))? {
            SubscriptionResult::Ok => Ok(()),
            SubscriptionResult::Duplicate => Err(anyhow!("duplicate subscriptions")),
            SubscriptionResult::NotFound => Err(anyhow!("subscriptions not found")),
        }?;

        myself.send_interval(Duration::from_secs(1), || ApplyVFS);

        Ok(FileActorState {
            inner_file_actor: fwactor.get_cell(),
            proj: dependency.proj,
            cache_actor: dependency.cache_actor,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            FileCreated(created, paths) => {
                if let Some(path) =
                    ValidPath::option(paths.first().cloned(), &state.proj.workspace())
                {
                    Self::handle_file_created(state.proj.clone(), created, path).await?;
                }
            }
            FileModified(modified, paths) => {
                let v_paths: Vec<_> = paths
                    .iter()
                    .flat_map(|x| ValidPath::new(x.clone(), &state.proj.workspace()))
                    .collect();
                if v_paths.len() == paths.len() {
                    Self::handle_file_modified(
                        state.proj.clone(),
                        modified,
                        v_paths,
                        state.cache_actor.clone(),
                    )
                    .await?
                }
            }
            FileRemoved(_, paths) => {
                if let Some(v_path) =
                    ValidPath::option(paths.first().cloned(), &state.proj.workspace())
                {
                    Self::handle_file_deletion(
                        state.proj.clone(),
                        v_path,
                        state.cache_actor.clone(),
                    )
                    .await?;
                }
            }
            ApplyVFS => {
                state.proj.apply_vfs_changes()?;
                state.cache_actor.send_message(ApplyChanges)?;
            }
        }
        Ok(())
    }
}

impl FileActor {
    async fn handle_file_created(
        proj: RustProject,
        create_kind: CreateKind,
        path: ValidPath,
    ) -> Result<(), ActorProcessingErr> {
        match create_kind {
            CreateKind::File => {
                Self::read_and_apply_vfs(path, proj.clone()).await?;
            }
            CreateKind::Folder => {
                let mut vfs = proj.vfs.lock().unwrap();
                let path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
                vfs.set_file_contents(path, None);
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_file_deletion(
        proj: RustProject,
        path: ValidPath,
        cache_actor: ActorRef<cache_actor::Message>,
    ) -> Result<(), ActorProcessingErr> {
        let mut vfs = proj.vfs.lock().unwrap();

        let vfs_path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
        vfs.set_file_contents(vfs_path, None);
        cache_actor.send_message(cache_actor::Message::InvalidateFile(path))?;
        Ok(())
    }

    async fn handle_file_modified(
        proj: RustProject,
        modify_kind: ModifyKind,
        paths: Vec<ValidPath>,
        cache_actor: ActorRef<cache_actor::Message>,
    ) -> Result<(), ActorProcessingErr> {
        match modify_kind {
            ModifyKind::Data(_) => {
                if let Some(path) = paths.first().cloned() {
                    Self::read_and_apply_vfs(path.clone(), proj).await?;
                    cache_actor.send_message(cache_actor::Message::InvalidateFile(path))?;
                }
            }
            ModifyKind::Name(rename_mode) => {
                Self::handle_file_rename(proj, rename_mode, &paths).await?;
                if let Some(path) = paths.first().cloned() {
                    cache_actor.send_message(cache_actor::Message::InvalidateFile(path))?;
                }
            }
            ModifyKind::Metadata(_) => {}
            ModifyKind::Any => {}
            ModifyKind::Other => {}
        }
        Ok(())
    }

    async fn handle_file_rename(
        proj: RustProject,
        rename_mode: RenameMode,
        paths: &Vec<ValidPath>,
    ) -> Result<(), ActorProcessingErr> {
        match rename_mode {
            RenameMode::From => {
                paths.into_iter().for_each(|path| {
                    let mut vfs = proj.vfs.lock().unwrap();
                    let vfs_path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
                    vfs.set_file_contents(vfs_path, None);
                });
            }
            RenameMode::To => {
                if let Some(path) = paths.first().cloned() {
                    Self::read_and_apply_vfs(path, proj.clone()).await?;
                }
            }
            RenameMode::Both => {
                let mut iter = paths.iter().take(2);
                if let Some(first) = iter.next()
                    && let Some(second) = iter.next()
                {
                    let old_vfs = VfsPath::new_real_path(first.path.to_string_lossy().to_string());
                    let new_vfs = VfsPath::new_real_path(second.path.to_string_lossy().to_string());

                    let content = Self::read_file(&proj, second.path.clone()).await?;

                    let mut vfs = proj.vfs.lock().unwrap();

                    vfs.set_file_contents(old_vfs, None);
                    vfs.set_file_contents(new_vfs, Some(content));
                }
            }
            RenameMode::Any | RenameMode::Other => {}
        }
        Ok(())
    }

    async fn read_and_apply_vfs(
        path: ValidPath,
        proj: RustProject,
    ) -> Result<(), ActorProcessingErr> {
        let content = Self::read_file(&proj, path.path.clone()).await?;
        let path = VfsPath::new_real_path(path.path.to_string_lossy().into_owned());
        proj.vfs
            .lock()
            .unwrap()
            .set_file_contents(path, Some(content));
        Ok(())
    }

    async fn read_file(proj: &RustProject, path: PathBuf) -> anyhow::Result<Vec<u8>> {
        let workspace = proj.workspace();
        tokio::task::spawn_blocking(move || workspace.read(&path).map(String::into_bytes)).await?
    }
}

struct Forwarder {
    actor: ActorCell,
}
impl FileWatcherSubscriber for Forwarder {
    fn event_received(&self, ev: notify_types::event::Event) {
        let res = match ev.kind {
            EventKind::Create(create) => self.actor.send_message(FileCreated(create, ev.paths)),
            EventKind::Modify(modify) => self.actor.send_message(FileModified(modify, ev.paths)),
            EventKind::Remove(remove) => self.actor.send_message(FileRemoved(remove, ev.paths)),
            _ => Ok(()),
        };
        match res {
            Ok(_) => (),
            Err(err) => {
                error!("{err}")
            }
        }
    }
}
