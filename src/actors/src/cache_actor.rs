use crate::file_actor::ValidPath;
use analysis::cache::TypedCache;
use analysis::rust_proj::RustProject;
use analysis::symbol_info::SymbolInfo;
use analysis::utils::RPath;
use ractor::{Actor, ActorProcessingErr, ActorRef};

pub struct CacheActor {}

pub struct Dependency {
    pub proj: RustProject,
    pub symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
}

pub struct CacheActorState {
    proj: RustProject,
    symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
    buffer: Vec<ValidPath>,
}

#[derive(Debug)]
pub enum Message {
    InvalidateFile(ValidPath),
    ApplyChanges,
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
            proj: dependency.proj,
            symbol_cache: dependency.symbol_cache,
            buffer: Vec::new(),
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
                state.buffer.push(file);
            }
            Message::ApplyChanges => {
                let buffer: Vec<_> = state.buffer.drain(..).collect();
                let session = state.proj.new_analysis().await;
                for x in buffer {
                    let _ = state.symbol_cache.transaction(|db| {
                        let rpath = RPath::new(x.path.clone(), state.proj.root.clone())?;
                        let remove_keys: Vec<_> = db.prefix_iter(rpath.inner)?.collect();

                        remove_keys.iter().try_for_each(|(_, v)| db.delete(v))?;

                        let nodes = if let Some(f_id) = state.proj.get_file_id(x.path.clone()) {
                            let file_structs = session.get_file_structure(f_id);
                            let line_ind = session.get_line_indecies(f_id)?;
                            SymbolInfo::from_file_structs(
                                f_id,
                                file_structs,
                                x.path.clone(),
                                line_ind,
                                &state.proj.root,
                            )?
                        } else {
                            Vec::new()
                        };
                        nodes.iter().try_for_each(|s| db.put(s, s))?;
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
}
