use crate::file_actor::Message::{FileCreated, FileModified, FileRemoved};
use anyhow::anyhow;
use futures::future::OptionFuture;
use notify_types::event::{CreateKind, Event, EventKind, ModifyKind, RemoveKind, RenameMode};
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::{Vfs, VfsPath};
use ractor::{Actor, ActorProcessingErr, ActorRef, MessagingErr, SupervisionEvent, call};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct FileActor {}

pub struct Dependency {
    vfs: Arc<Mutex<Vfs>>,
    a_host: Arc<Mutex<AnalysisHost>>,
}

pub struct FileActorState {
    vfs: Arc<Mutex<Vfs>>,
    a_host: Arc<Mutex<AnalysisHost>>,
    inner_file_actor: ActorRef<FileWatcherMessage>,
}

#[derive(Debug)]
pub enum Message {
    FileCreated(CreateKind, Vec<PathBuf>),
    FileModified(ModifyKind, Vec<PathBuf>),
    FileRemoved(RemoveKind, Vec<PathBuf>),
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
            ..Default::default()
        };
        let (fwactor, fwhandle) = Actor::spawn(None, fw, config)
            .await
            .expect("Filewatcher failed to spawn");

        let fwrder = Forwarder {
            actor: myself.clone(),
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
        Ok(FileActorState {
            inner_file_actor: fwactor,
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
                Self::handle_file_created(state.vfs.clone(), created, paths).await?;
            }
            FileModified(modified, paths) => {
                Self::handle_file_modified(state.vfs.clone(), modified, paths).await?;
            }
            FileRemoved(_, paths) => {
                let mut vfs = state.vfs.lock().unwrap();
                paths.into_iter().for_each(|path| {
                    let vfs_path = VfsPath::new_real_path(path.to_string_lossy().to_string());
                    vfs.set_file_contents(vfs_path, None);
                });
            }
        }
        Ok(())
    }
}

impl FileActor {
    async fn handle_file_created(
        vfs: Arc<Mutex<Vfs>>,
        create_kind: CreateKind,
        paths: Vec<PathBuf>,
    ) -> Result<(), ActorProcessingErr> {
        match create_kind {
            CreateKind::File => {
                Self::read_and_apply_vfs(paths, vfs.clone()).await?;
            }
            CreateKind::Folder => {
                paths.first().map(|path_buf| {
                    let mut vfs = vfs.lock().unwrap();
                    let path = VfsPath::new_real_path(path_buf.to_string_lossy().to_string());
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
        paths: Vec<PathBuf>,
    ) -> Result<(), ActorProcessingErr> {
        match modify_kind {
            ModifyKind::Data(_) => {
                Self::read_and_apply_vfs(paths, vfs).await?;
            }
            ModifyKind::Name(rename_mode) => {
                Self::handle_file_rename(vfs, rename_mode, paths).await?;
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
        paths: Vec<PathBuf>,
    ) -> Result<(), ActorProcessingErr> {
        match rename_mode {
            RenameMode::From => {
                paths.into_iter().for_each(|path| {
                    let mut vfs = vfs.lock().unwrap();
                    let vfs_path = VfsPath::new_real_path(path.to_string_lossy().to_string());
                    vfs.set_file_contents(vfs_path, None);
                });
            }
            RenameMode::To => {
                Self::read_and_apply_vfs(paths, vfs.clone()).await?;
            }
            RenameMode::Both => {
                let mut iter = paths.iter().take(2);
                if let Some(first) = iter.next()
                    && let Some(second) = iter.next()
                {
                    let old_vfs = VfsPath::new_real_path(first.to_string_lossy().to_string());
                    let new_vfs = VfsPath::new_real_path(second.to_string_lossy().to_string());

                    let content = tokio::fs::read(&second).await?;

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
        paths: Vec<PathBuf>,
        vfs: Arc<Mutex<Vfs>>,
    ) -> Result<(), ActorProcessingErr> {
        let content = OptionFuture::from(
            paths
                .first()
                .cloned()
                .map(|path_buf: PathBuf| tokio::fs::read(path_buf)),
        )
        .await
        .transpose()?;

        paths.first().map(|path_buf| {
            let path = VfsPath::new_real_path(path_buf.to_string_lossy().to_string());
            let mut vfs = vfs.lock().unwrap();
            vfs.set_file_contents(path, content)
        });
        Ok(())
    }
}

struct Forwarder {
    actor: ActorRef<Message>,
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
            Err(_) => {}
        }
    }
}
