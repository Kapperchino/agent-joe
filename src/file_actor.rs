use crate::file_actor::Message::{ApplyVFS, FileCreated, FileModified, FileRemoved};
use anyhow::anyhow;
use futures::future::OptionFuture;
use itertools::cloned;
use notify_types::event::{CreateKind, Event, EventKind, ModifyKind, RemoveKind, RenameMode};
use ra_ap_ide::AnalysisHost;
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_vfs::{Change, ChangeKind, Vfs, VfsPath};
use ractor::concurrency::Duration;
use ractor::{
    call, Actor, ActorCell, ActorProcessingErr, ActorRef, MessagingErr, SupervisionEvent,
};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct FileActor {}

pub struct Dependency {
    pub(crate) vfs: Arc<Mutex<Vfs>>,
    pub(crate) a_host: Arc<Mutex<AnalysisHost>>,
    pub main_dir: PathBuf,
}

pub struct FileActorState {
    vfs: Arc<Mutex<Vfs>>,
    a_host: Arc<Mutex<AnalysisHost>>,
    inner_file_actor: ActorCell,
}

#[derive(Clone)]
pub struct ValidPath {
    path: PathBuf,
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
            directories: vec![dependency.main_dir.join("src")],
            files: vec![dependency.main_dir.join("Cargo.toml")],
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
                if !paths.iter().any(|x| {
                    if let Some(p) = x.to_str()
                        && !p.ends_with("~")
                    {
                        true
                    } else {
                        false
                    }
                }) {
                    Self::handle_file_created(
                        state.vfs.clone(),
                        created,
                        paths.first().cloned().and_then(|t| ValidPath::new(t)),
                    )
                    .await?;
                }
            }
            FileModified(modified, paths) => {
                let paths = if !paths.iter().any(|p| p.ends_with("~")) {
                    Some(
                        paths
                            .into_iter()
                            .flat_map(|x1| ValidPath::new(x1))
                            .collect(),
                    )
                } else {
                    None
                };
                match Self::handle_file_modified(state.vfs.clone(), modified, paths).await {
                    Ok(_) => Ok(()),
                    Err(err) => {
                        log::error!("{err}");
                        Err(err)
                    }
                }?
            }
            FileRemoved(_, paths) => {
                let mut vfs = state.vfs.lock().unwrap();
                paths.into_iter().for_each(|path| {
                    let vfs_path = VfsPath::new_real_path(path.to_string_lossy().to_string());
                    vfs.set_file_contents(vfs_path, None);
                });
            }
            ApplyVFS => {
                let changes = { state.vfs.lock().unwrap().take_changes() };
                let mut proj_change = ChangeWithProcMacros::default();

                changes
                    .into_iter()
                    .for_each(|(id, change)| match change.change {
                        Change::Create(contents, _hash) | Change::Modify(contents, _hash) => {
                            let text = String::from_utf8(contents).ok();
                            proj_change.change_file(id, text.map(Into::into));
                        }
                        Change::Delete => {
                            proj_change.change_file(id, None);
                        }
                    });
                state.a_host.lock().unwrap().apply_change(proj_change);
            }
        }
        Ok(())
    }
}

impl FileActor {
    async fn handle_file_created(
        vfs: Arc<Mutex<Vfs>>,
        create_kind: CreateKind,
        path: Option<ValidPath>,
    ) -> Result<(), ActorProcessingErr> {
        match create_kind {
            CreateKind::File => {
                Self::read_and_apply_vfs(path, vfs.clone()).await?;
            }
            CreateKind::Folder => {
                path.map(|path_buf| {
                    let mut vfs = vfs.lock().unwrap();
                    let path = VfsPath::new_real_path(path_buf.path.to_string_lossy().to_string());
                    vfs.set_file_contents(path, None);
                });
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_file_modified(
        vfs: Arc<Mutex<Vfs>>,
        modify_kind: ModifyKind,
        paths: Option<Vec<ValidPath>>,
    ) -> Result<(), ActorProcessingErr> {
        match paths {
            Some(paths) => match modify_kind {
                ModifyKind::Data(_) => {
                    Self::read_and_apply_vfs(paths.first().cloned(), vfs).await?;
                }
                ModifyKind::Name(rename_mode) => {
                    Self::handle_file_rename(vfs, rename_mode, paths).await?;
                }
                ModifyKind::Metadata(_) => {}
                ModifyKind::Any => {}
                ModifyKind::Other => {}
            },
            None => {}
        }
        Ok(())
    }

    async fn handle_file_rename(
        vfs: Arc<Mutex<Vfs>>,
        rename_mode: RenameMode,
        paths: Vec<ValidPath>,
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
                Self::read_and_apply_vfs(paths.first().cloned(), vfs.clone()).await?;
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
        path: Option<ValidPath>,
        vfs: Arc<Mutex<Vfs>>,
    ) -> Result<(), ActorProcessingErr> {
        let content = OptionFuture::from(path.clone().map(|path| tokio::fs::read(path.path)))
            .await
            .transpose()?;

        path.map(|path| {
            let path = VfsPath::new_real_path(path.path.to_string_lossy().to_string());
            let mut vfs = vfs.lock().unwrap();
            vfs.set_file_contents(path, content)
        });
        Ok(())
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
                log::error!("{err}")
            }
        }
    }
}
