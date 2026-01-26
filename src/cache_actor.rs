use crate::analysis::SymbolInfo;
use crate::cache::{CacheKey, TypedCache};
use crate::file_actor::ValidPath;
use crate::rust_proj::RustProject;
use itertools::Itertools;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::{Vfs, VfsPath};
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

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
                buffer.iter().try_for_each(|x| {
                    state.symbol_cache.transaction(|db| {
                        let remove_keys: Vec<_> = db
                            .prefix_iter(x.path.to_string_lossy().to_string())?
                            .collect();
                        remove_keys.iter().try_for_each(|(_, v)| db.delete(v))?;
                        let nodes = if let Some(f_id) = state.proj.get_file_id(x.path.clone()) {
                            let file_structs = session.get_file_structure(f_id);
                            SymbolInfo::from_file_structs(f_id, file_structs, x.path.clone())
                        } else {
                            Vec::new()
                        };
                        nodes.iter().try_for_each(|s| db.put(s, s))?;
                        let (_, infos): (Vec<_>, Vec<_>) = remove_keys.into_iter().unzip();
                        let infos: HashMap<String, SymbolInfo> =
                            infos.into_iter().map(|x1| (x1.get_key(), x1)).collect();
                        let nodes: HashMap<String, SymbolInfo> =
                            nodes.into_iter().map(|x1| (x1.get_key(), x1)).collect();
                        infos.iter().for_each(|(info, node)| {
                            match nodes.get(info) {
                                None => {}
                                Some(other) => {
                                    println!("{:?}", other);
                                    println!("{:?}", info);
                                }
                            };
                        });
                        Ok(())
                    })?;
                    Ok::<(), anyhow::Error>(())
                })?;
            }
        }
        Ok(())
    }
}
