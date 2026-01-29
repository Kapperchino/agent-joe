use crate::cache::{TypedCache, TypedCacheDb};
use crate::file_meta::{FileMeta, FileMetaData};
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
use crate::utils::Utils;
use anyhow::anyhow;
use futures::{future, StreamExt};
use itertools::Itertools;
use ra_ap_vfs::FileId;
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
    pub file_cache: TypedCache<FileMetaData, FileMetaData>,
    pub file_metas: HashMap<String, FileMeta>,
}

impl CurContext {
    pub async fn new() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let mut file_cache = TypedCache::new(None).await?;
        let hashes: HashMap<_, _> = Self::get_file_hashes(&proj).await?.into_iter().collect();
        let mut file_metas = Self::get_file_metas(&mut file_cache, &proj, &hashes).await?;
        let _ = Self::validate_and_update_cache(hashes, &mut file_metas, &mut file_cache, &proj)
            .await?;

        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            file_cache,
            file_metas,
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
        file_metas: &mut HashMap<String, FileMeta>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
        proj: &RustProject,
    ) -> anyhow::Result<()> {
        let files_to_redo: HashSet<_> = hashes
            .iter()
            .flat_map(|(k, v)| {
                let p = k.to_string_lossy();
                match file_metas.get(&p.to_string()) {
                    Some(file) => {
                        if &file.data.hash != v {
                            Some(k.clone())
                        } else {
                            None
                        }
                    }
                    None => Some(k.clone()),
                }
            })
            .collect();

        let hashes: HashMap<_, _> = hashes
            .into_iter()
            .filter(|(k, _)| files_to_redo.contains(k))
            .collect();

        let file_meta_datas = Self::invalidate_and_redo_cache(
            files_to_redo.into_iter().collect_vec(),
            &proj,
            hashes,
            cache,
        )
        .await?;

        file_metas.extend(Self::get_file_metas_inner(&proj, file_meta_datas).await?);
        Ok(())
    }

    pub async fn invalidate_and_redo_cache(
        paths: Vec<PathBuf>,
        proj: &RustProject,
        hashes: HashMap<PathBuf, Vec<u8>>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMetaData>> {
        let session = proj.new_analysis().await;

        let meta = paths.into_iter().try_fold(HashMap::new(), |mut acc, p| {
            let nodes = if let Some(f_id) = proj.get_file_id(p.clone()) {
                let file_structs = session.get_file_structure(f_id);
                SymbolInfo::from_file_structs(f_id, file_structs, p.clone())
            } else {
                Vec::new()
            };
            let meta_data = FileMetaData::get_file_meta_datas_cache_miss(nodes, proj, &hashes)?;
            acc.extend(meta_data);
            Ok::<HashMap<String, FileMetaData>, anyhow::Error>(acc)
        })?;

        cache.transaction(|db| {
            meta.iter().try_for_each(|(_, s)| db.put(s, s))?;
            Ok(())
        })?;

        Ok(meta)
    }

    pub async fn get_file_meta_datas(
        rust_proj: &RustProject,
        hashes: &HashMap<PathBuf, Vec<u8>>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMetaData>> {
        let is_empty = cache.read_transaction(|db| db.is_empty())?;
        if is_empty {
            let symbols = rust_proj.get_all_proj_symbols().await?;
            cache.transaction(|db: &mut TypedCacheDb<_, _>| {
                let metas =
                    FileMetaData::get_file_meta_datas_cache_miss(symbols, rust_proj, hashes)?;
                let (_, errs): (Vec<_>, Vec<_>) = metas
                    .iter()
                    .map(|(_, v)| db.put(v, v))
                    .partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(metas)
            })
        } else {
            cache.read_transaction(|db| {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res
                    .into_iter()
                    .map(|data| (data.rpath.clone(), data))
                    .collect())
            })
        }
    }

    pub async fn get_file_metas_inner(
        rust_proj: &RustProject,
        file_meta_datas: HashMap<String, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMeta>> {
        let session = rust_proj.new_analysis().await;
        let res: Result<HashMap<_, _>, _> = file_meta_datas
            .into_iter()
            .map(|(k, v)| {
                session
                    .get_line_indecies(FileId::from_raw(v.file_id))
                    .map(|line_index| {
                        let meta = FileMeta {
                            line_index,
                            data: v,
                        };
                        (k, meta)
                    })
            })
            .collect();
        let res = res?;
        Ok(res)
    }

    async fn get_file_metas(
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
        rust_proj: &RustProject,
        hashes: &HashMap<PathBuf, Vec<u8>>,
    ) -> anyhow::Result<HashMap<String, FileMeta>> {
        let datas = Self::get_file_meta_datas(rust_proj, hashes, cache).await?;
        Self::get_file_metas_inner(rust_proj, datas).await
    }

    pub fn format_file_metas(metas: &HashMap<String, FileMeta>) -> String {
        metas
            .values()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub async fn get_file_hashes(proj: &RustProject) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        let contents = Self::get_files(proj).await?;
        let results: Vec<_> = futures::stream::iter(contents)
            .map(|(path, content)| {
                tokio::spawn(
                    async move { (path, blake3::hash(content.as_bytes()).as_bytes().to_vec()) },
                )
            })
            .buffer_unordered(100)
            .collect::<Vec<_>>()
            .await;

        let res = results.into_iter().collect::<Result<Vec<_>, _>>()?;
        Ok(res)
    }

    pub async fn get_file_hashes_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        let contents = Self::get_files_for_paths(paths).await?;
        let results: Vec<_> = futures::stream::iter(contents)
            .map(|(path, content)| {
                tokio::spawn(
                    async move { (path, blake3::hash(content.as_bytes()).as_bytes().to_vec()) },
                )
            })
            .buffer_unordered(100)
            .collect::<Vec<_>>()
            .await;

        let res = results.into_iter().collect::<Result<Vec<_>, _>>()?;
        Ok(res)
    }

    async fn get_files(proj: &RustProject) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let anal = proj.new_analysis().await;
        future::join_all(anal.get_work_files().into_iter().flat_map(|file| {
            file.path.into_abs_path().map(async |path| {
                Utils::get_file_content(&path.clone().into())
                    .await
                    .map(|content| (path.into(), content))
            })
        }))
        .await
        .into_iter()
        .collect()
    }

    async fn get_files_for_paths(paths: Vec<PathBuf>) -> anyhow::Result<Vec<(PathBuf, String)>> {
        future::join_all(paths.into_iter().map(async |file| {
            Utils::get_file_content(&file)
                .await
                .map(|content| (file, content))
        }))
        .await
        .into_iter()
        .collect()
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
