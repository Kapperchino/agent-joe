use crate::cache::TypedCache;
use crate::contexts::context::{Context, LineIndexCreator};
use crate::proj_meta::ProjMeta;
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
use crate::utils::RPath;
use anyhow::anyhow;
use async_trait::async_trait;
use itertools::Itertools;
use ra_ap_ide::LineIndex;
use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use triomphe;
use utils::files::Files;
use utils::utils::{FnvHashMap, Utils};

pub struct RustContextLineIndexCreator {
    pub(crate) proj_meta: Arc<ProjMeta>,
}

impl LineIndexCreator for RustContextLineIndexCreator {
    fn create_index(&self, file_path: &PathBuf) -> anyhow::Result<triomphe::Arc<LineIndex>> {
        match self
            .proj_meta
            .files
            .get(&file_path.to_string_lossy().to_string())
        {
            Some(meta) => Ok(meta.line_index.clone()),
            None => Err(anyhow!("File not found!")),
        }
    }
}

#[async_trait]
impl Context for RustContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let analytical_ctx = self.get_analytical_context().await.unwrap_or_default();
        let init_prompt = self.initial_prompt.as_str();
        let stacked_prompt = self.stacked_context.join("\n");
        format!("{init_prompt} \n project_root: {dir}\n{analytical_ctx}\n{stacked_prompt}")
    }

    fn get_root(&self) -> PathBuf {
        self.cur_dir.clone()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(self
            .get_proj_meta()
            .await?
            .files
            .keys()
            .map(PathBuf::from)
            .collect())
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        let proj = self.get_proj_meta().await?;
        Ok(Box::new(RustContextLineIndexCreator {
            proj_meta: Arc::new(proj),
        }))
    }

    fn gen_id(&self) -> u64 {
        self.id_gen.fetch_add(1, Ordering::AcqRel) + 1
    }
}

#[derive(Clone)]
pub struct RustContext {
    pub cur_dir: PathBuf,
    pub rust_proj: RustProject,
    pub symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
    pub initial_prompt: String,
    pub stacked_context: Vec<String>,
    pub id_gen: Arc<AtomicU64>,
}

impl RustContext {
    pub async fn new(initial_prompt: String, id: u64) -> Result<RustContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Files::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let store_dir = Utils::get_store_dir()?;
        let mut cache = TypedCache::new(store_dir).await?;
        let hashes: FnvHashMap<_, _> = ProjMeta::get_file_hashes(&proj)
            .await?
            .into_iter()
            .collect();
        //validate old one
        let proj_meta = Self::get_proj_meta_init(&mut cache, &proj).await?;
        let _ = Self::validate_and_update_cache(hashes, &proj_meta, &mut cache, &proj).await?;

        Ok(RustContext {
            cur_dir: current_dir,
            initial_prompt,
            rust_proj: proj,
            symbol_cache: cache,
            stacked_context: Vec::new(),
            id_gen: Arc::new(AtomicU64::new(id)),
        })
    }

    pub async fn get_analytical_context(&self) -> anyhow::Result<String> {
        let meta = self.get_proj_meta().await?;
        Ok(meta.to_string())
    }

    pub async fn validate_and_update_cache(
        hashes: FnvHashMap<PathBuf, Vec<u8>>,
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

        let meta = paths.iter().try_fold(Vec::new(), |mut acc, p| {
            let nodes = if let Some(f_id) = session.get_file_id(p) {
                let file_structs = session.get_file_structure(f_id);
                let line_ind = session.get_line_indecies(f_id)?;
                SymbolInfo::from_file_structs(f_id, file_structs, p.clone(), line_ind, &proj.root)
            } else {
                Ok(Vec::new())
            }?;
            acc.extend(nodes);
            Ok::<Vec<_>, anyhow::Error>(acc)
        })?;

        cache.transaction(|db| {
            paths.iter().try_for_each(|path| {
                let rpath = RPath::new(path.clone(), proj.root.clone())?;
                let iter = db.prefix_iter(rpath.inner)?;
                let invalidates: Vec<_> = iter.collect();
                invalidates.iter().try_for_each(|(_, v)| db.delete(v))
            })?;
            meta.iter().try_for_each(|s| db.put(s, s))?;
            Ok(())
        })?;

        Ok(meta)
    }

    // cache already exists
    pub async fn get_proj_meta(&self) -> anyhow::Result<ProjMeta> {
        let symbols = SymbolInfo::get_symbols_from_cache(&self.symbol_cache).await?;
        let res = ProjMeta::get_proj_meta_from_symbols(symbols, &self.rust_proj).await?;
        Ok(res)
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        let char_count = text.chars().count();
        (char_count + 3) / 4
    }

    async fn get_proj_meta_init(
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<ProjMeta> {
        let symbols = SymbolInfo::get_symbols_with_cache_write(rust_proj, cache).await?;
        let res = ProjMeta::get_proj_meta_from_symbols(symbols, rust_proj).await?;
        Ok(res)
    }
}
