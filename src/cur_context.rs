use crate::cache::TypedCache;
use crate::proj_meta::ProjMeta;
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
use crate::utils::Utils;
use futures::StreamExt;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::Display;
use std::path::PathBuf;
use tokio::fs::DirEntry;

pub struct CurContext {
    pub cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    pub rust_proj: RustProject,
    pub symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
    pub proj_meta: ProjMeta,
}

impl CurContext {
    pub async fn new() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let mut cache = TypedCache::new(None).await?;
        let hashes: HashMap<_, _> = ProjMeta::get_file_hashes(&proj)
            .await?
            .into_iter()
            .collect();
        //validate old one
        let proj_meta = Self::get_proj_meta(&mut cache, &proj).await?;
        let proj_meta =
            match Self::validate_and_update_cache(hashes, &proj_meta, &mut cache, &proj).await? {
                true => {
                    // updated version
                    Self::get_proj_meta(&mut cache, &proj).await?
                }
                false => proj_meta,
            };

        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            symbol_cache: cache,
            proj_meta,
        })
    }

    pub async fn get_ctx(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let analytical_ctx = self.get_analytical_context().await.unwrap_or_default();
        format!(
            "# Current Context: \ncurrent directory: {dir}\n## Analytical Context: \nThe offset range are given along with the symbol information\n{analytical_ctx}"
        )
    }

    pub async fn get_analytical_context(&self) -> anyhow::Result<String> {
        Ok(Self::format_file_metas(&self.file_metas))
    }

    pub async fn validate_and_update_cache(
        hashes: HashMap<PathBuf, Vec<u8>>,
        proj_meta: &ProjMeta,
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
        proj: &RustProject,
    ) -> anyhow::Result<bool> {
        let files_to_redo: HashSet<_> = hashes
            .iter()
            .flat_map(|(k, v)| {
                let p = k.to_string_lossy();
                match proj_meta.files.get(&p.to_string()) {
                    Some(file) => {
                        if &file.hash != v {
                            Some(k.clone())
                        } else {
                            None
                        }
                    }
                    None => Some(k.clone()),
                }
            })
            .collect();

        if files_to_redo.is_empty() {
            Ok(false)
        } else {
            let _ = Self::invalidate_and_redo_cache(
                files_to_redo.into_iter().collect_vec(),
                &proj,
                cache,
            )
            .await?;
            Ok(true)
        }
    }

    pub async fn invalidate_and_redo_cache(
        paths: Vec<PathBuf>,
        proj: &RustProject,
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = proj.new_analysis().await;

        let meta = paths.into_iter().try_fold(Vec::new(), |mut acc, p| {
            let nodes = if let Some(f_id) = proj.get_file_id(p.clone()) {
                let file_structs = session.get_file_structure(f_id);
                SymbolInfo::from_file_structs(f_id, file_structs, p.clone())
            } else {
                Vec::new()
            };
            acc.extend(nodes);
            Ok::<Vec<_>, anyhow::Error>(acc)
        })?;

        cache.transaction(|db| {
            meta.iter().try_for_each(|s| db.delete(s))?;
            meta.iter().try_for_each(|s| db.put(s, s))?;
            Ok(())
        })?;

        Ok(meta)
    }

    async fn get_proj_meta(
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<ProjMeta> {
        let symbols = SymbolInfo::get_symbols_with_cache(rust_proj, cache).await?;
        let res = ProjMeta::get_proj_meta_from_symbols(symbols, rust_proj).await?;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_analytical_context() {
        let mut ctx = CurContext::new()
            .await
            .expect("Failed to create CurContext");
        let context = ctx.get_ctx().await;
        println!("\n=== Analytical Context ===\n{}", context);
    }
}
