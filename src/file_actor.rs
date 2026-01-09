use crate::file_actor::Message::{FileCreated, FileModified, FileRemoved};
use anyhow::anyhow;
use notify_types::event::{CreateKind, Event, EventKind, ModifyKind, RemoveKind};
use ra_ap_vfs::Vfs;
use ractor::{Actor, ActorProcessingErr, ActorRef, MessagingErr, SupervisionEvent, call};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct FileActor {}

pub struct Dependency {
    vfs: Arc<Mutex<Vfs>>,
}

pub struct FileActorState {
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
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            FileCreated(created, paths) => match created {
                CreateKind::Any => {}
                CreateKind::File => {}
                CreateKind::Folder => {}
                CreateKind::Other => {}
            },
            FileModified(modified, paths) => match modified {
                ModifyKind::Any => {}
                ModifyKind::Data(_) => {}
                ModifyKind::Metadata(data) => {}
                ModifyKind::Name(name) => {}
                ModifyKind::Other => {}
            },
            FileRemoved(removed, paths) => {}
        }
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
