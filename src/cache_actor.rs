use crate::analysis::SymbolInfo;
use crate::cache::TypedCache;
use crate::cur_context::{CurContext, FileMeta, FileMetaData};
use crate::file_actor::ValidPath;
use crate::rust_proj::RustProject;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use std::path::PathBuf;

pub struct CacheActor {}

pub struct Dependency {
    pub proj: RustProject,
    pub file_cache: TypedCache<FileMetaData, FileMetaData>,
}

pub struct CacheActorState {
    proj: RustProject,
    file_cache: TypedCache<FileMetaData, FileMetaData>,
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
            file_cache: dependency.file_cache,
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
                    let deleted_paths = state.file_cache.transaction(|db| {
                        let remove_keys: Vec<_> = db
                            .prefix_iter(x.path.to_string_lossy().to_string())?
                            .collect();
                        remove_keys.iter().try_for_each(|(_, v)| db.delete(v))?;
                        let deleted_paths: Vec<PathBuf> = remove_keys
                            .into_iter()
                            .map(|(_, meta)| meta.rpath.into())
                            .collect();

                        Ok(deleted_paths)
                    })?;

                    let nodes = if let Some(f_id) = state.proj.get_file_id(x.path.clone()) {
                        let file_structs = session.get_file_structure(f_id);
                        SymbolInfo::from_file_structs(f_id, file_structs, x.path.clone())
                    } else {
                        Vec::new()
                    };

                    let hashes = CurContext::get_file_hashes_for_paths(deleted_paths)
                        .await?
                        .into_iter()
                        .collect();
                    let meta_data =
                        CurContext::get_file_meta_datas_cache_miss(nodes, &state.proj, hashes)?;

                    state.file_cache.transaction(|db| {
                        meta_data.iter().try_for_each(|(_, s)| db.put(s, s))?;
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
}
