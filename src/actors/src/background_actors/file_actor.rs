use crate::background_actors::cache_actor;
use crate::background_actors::cache_actor::Message::ApplyChanges;
use crate::background_actors::file_actor::Message::{
    ApplyVFS, FileCreated, FileModified, FileRemoved,
};
use anyhow::anyhow;
use futures::future::OptionFuture;
use itertools::cloned;
use notify_types::event::{CreateKind, Event, EventKind, ModifyKind, RemoveKind, RenameMode};
use ra_ap_ide::{AnalysisHost, SourceRoot};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_vfs::file_set::FileSet;
use ra_ap_vfs::{Change, ChangeKind, FileId, Vfs, VfsPath};
use ractor::concurrency::Duration;
use ractor::{
    call, Actor, ActorCell, ActorProcessingErr, ActorRef, MessagingErr, SupervisionEvent,
};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::error;

pub struct FileActor {}

pub struct Dependency {
    pub(crate) vfs: Arc<Mutex<Vfs>>,
    pub(crate) a_host: Arc<Mutex<AnalysisHost>>,
    pub main_dir: PathBuf,
    pub cache_actor: ActorRef<cache_actor::Message>,
}

pub struct FileActorState {
    vfs: Arc<Mutex<Vfs>>,
    a_host: Arc<Mutex<AnalysisHost>>,
    inner_file_actor: ActorCell,
    cache_actor: ActorRef<cache_actor::Message>,
}

#[derive(Clone, Debug)]
pub struct ValidPath {
    pub(crate) path: PathBuf,
}

impl ValidPath {
    pub fn new(path: PathBuf) -> Option<Self> {
        if let Some(joe) = path.to_str()
            && !joe.ends_with("~")
        {
            Some(ValidPath { path })
        } else {
            None
        }
    }

    pub fn option(path: Option<PathBuf>) -> Option<Self> {
        path.and_then(|path| Self::new(path))
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
        // spawning file watcher actor, forwarding messages to our own actor
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
            vfs: dependency.vfs,
            a_host: dependency.a_host,
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
                if let Some(path) = ValidPath::option(paths.first().cloned()) {
                    Self::handle_file_created(state.vfs.clone(), created, path).await?;
                }
            }
            FileModified(modified, paths) => {
                let v_paths: Vec<_> = paths
                    .iter()
                    .flat_map(|x| ValidPath::new(x.clone()))
                    .collect();
                if v_paths.len() == paths.len() {
                    Self::handle_file_modified(
                        state.vfs.clone(),
                        modified,
                        v_paths,
                        state.cache_actor.clone(),
                    )
                    .await?
                }
            }
            FileRemoved(_, paths) => {
                if let Some(v_path) = ValidPath::option(paths.first().cloned()) {
                    Self::handle_file_deletion(
                        state.vfs.clone(),
                        v_path,
                        state.cache_actor.clone(),
                    )
                    .await?;
                }
            }
            ApplyVFS => {
                let mut vfs = state.vfs.lock().unwrap();
                let changes = vfs.take_changes();
                let mut proj_change = ChangeWithProcMacros::default();
                let mut roots = Vec::new();
                drop(vfs);

                if !changes.is_empty() {
                    changes
                        .into_iter()
                        .for_each(|(id, change)| match change.change {
                            Change::Create(contents, _hash) | Change::Modify(contents, _hash) => {
                                let mut fs = FileSet::default();
                                let vfs = state.vfs.lock().unwrap();
                                fs.insert(id, vfs.file_path(id).clone());
                                roots.push(SourceRoot::new_local(fs));
                                let text = String::from_utf8(contents).ok();
                                proj_change.change_file(id, text.map(Into::into));
                            }
                            Change::Delete => {
                                proj_change.change_file(id, None);
                            }
                        });

                    proj_change.set_roots(roots);
                    state.a_host.lock().unwrap().apply_change(proj_change);
                    state.cache_actor.send_message(ApplyChanges)?;
                }
            }
        }
        Ok(())
    }
}

impl FileActor {
    async fn handle_file_created(
        vfs: Arc<Mutex<Vfs>>,
        create_kind: CreateKind,
        path: ValidPath,
    ) -> Result<(), ActorProcessingErr> {
        match create_kind {
            CreateKind::File => {
                Self::read_and_apply_vfs(path, vfs.clone()).await?;
            }
            CreateKind::Folder => {
                let mut vfs = vfs.lock().unwrap();
                let path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
                vfs.set_file_contents(path, None);
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_file_deletion(
        vfs: Arc<Mutex<Vfs>>,
        path: ValidPath,
        cache_actor: ActorRef<cache_actor::Message>,
    ) -> Result<(), ActorProcessingErr> {
        let mut vfs = vfs.lock().unwrap();

        let vfs_path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
        vfs.set_file_contents(vfs_path, None);
        cache_actor.send_message(cache_actor::Message::InvalidateFile(path))?;
        Ok(())
    }

    async fn handle_file_modified(
        vfs: Arc<Mutex<Vfs>>,
        modify_kind: ModifyKind,
        paths: Vec<ValidPath>,
        cache_actor: ActorRef<cache_actor::Message>,
    ) -> Result<(), ActorProcessingErr> {
        match modify_kind {
            ModifyKind::Data(_) => {
                if let Some(path) = paths.first().cloned() {
                    Self::read_and_apply_vfs(path.clone(), vfs).await?;
                    cache_actor.send_message(cache_actor::Message::InvalidateFile(path))?;
                }
            }
            ModifyKind::Name(rename_mode) => {
                Self::handle_file_rename(vfs, rename_mode, &paths).await?;
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
        vfs: Arc<Mutex<Vfs>>,
        rename_mode: RenameMode,
        paths: &Vec<ValidPath>,
    ) -> Result<(), ActorProcessingErr> {
        match rename_mode {
            RenameMode::From => {
                paths.into_iter().for_each(|path| {
                    let mut vfs = vfs.lock().unwrap();
                    let vfs_path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
                    vfs.set_file_contents(vfs_path, None);
                });
            }
            RenameMode::To => {
                if let Some(path) = paths.first().cloned() {
                    Self::read_and_apply_vfs(path, vfs.clone()).await?;
                }
            }
            RenameMode::Both => {
                let mut iter = paths.iter().take(2);
                if let Some(first) = iter.next()
                    && let Some(second) = iter.next()
                {
                    let old_vfs = VfsPath::new_real_path(first.path.to_string_lossy().to_string());
                    let new_vfs = VfsPath::new_real_path(second.path.to_string_lossy().to_string());

                    let content = tokio::fs::read(&second.path).await?;

                    let mut vfs = vfs.lock().unwrap();

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
        vfs: Arc<Mutex<Vfs>>,
    ) -> Result<(), ActorProcessingErr> {
        match path.path.exists() {
            true => {
                let content = tokio::fs::read(path.path.clone()).await?;
                let path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
                let mut vfs = vfs.lock().unwrap();
                vfs.set_file_contents(path, Some(content));
                Ok(())
            }
            false => Ok(()),
        }
    }
}

struct Forwarder {
    actor: ActorCell,
}
impl FileWatcherSubscriber for Forwarder {
    fn event_received(&self, ev: notify_types::event::Event) {
        // we only care about writes
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
