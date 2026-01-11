use crate::analysis::SymbolInfo;
use crate::cache::TypedCache;
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
    Actor, ActorCell, ActorProcessingErr, ActorRef, MessagingErr, SupervisionEvent, call,
};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct CacheActor {}

pub struct Dependency {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Arc<Mutex<Vfs>>,
    pub symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
}

pub struct CacheActorState {
    analysis_host: Arc<Mutex<AnalysisHost>>,
    vfs: Arc<Mutex<Vfs>>,
    symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
}

#[derive(Debug)]
pub enum Message {
    InvalidateFile(PathBuf),
}
#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for CacheActor {
    type Msg = Message;
    type State = CacheActorState;
    type Arguments = Dependency;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Dependency,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(CacheActorState {
            analysis_host: dependency.analysis_host,
            vfs: dependency.vfs,
            symbol_cache: dependency.symbol_cache,
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            Message::InvalidateFile(file) => {
                println!("{:?}", file)
            }
        }
        Ok(())
    }
}
