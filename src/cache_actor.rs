use crate::analysis::SymbolInfo;
use crate::cache::TypedCache;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use ractor::{
    Actor, ActorProcessingErr, ActorRef,
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
