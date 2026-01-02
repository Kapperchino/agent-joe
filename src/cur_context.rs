use crate::analysis::{AnalysisSession, SymbolInfo};
use crate::cache::{TypedCache, TypedCacheDb};
use crate::utils::Utils;
use anyhow::anyhow;
use itertools::Itertools;
use ra_ap_ide::{AnalysisHost, TextSize};
use ra_ap_ide_db::SymbolKind;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs::DirEntry;

pub struct CurContext {
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    rust_proj: RustProject,
    symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
}
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Vfs,
    pub files: HashMap<FileId, VfsPath>,
}
impl CurContext {
    pub async fn new() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let symbol_cache = TypedCache::new(None).await;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            symbol_cache,
        })
    }

    pub async fn to_string(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let session = self.rust_proj.new_analysis().await;
        let files: String = session
            .get_work_files()
            .into_iter()
            .map(|info| info.path.to_string())
            .fold(String::new(), |acc, s| format!("{acc}{s}").to_string());
        format!("Current Context: \ncurrent directory: {dir}\ncurrent files:\n{files}")
    }

    pub async fn get_analytical_context(&mut self) -> anyhow::Result<String> {
        let cache = &mut self.symbol_cache;
        let symbols = self.rust_proj.get_all_proj_symbols(cache).await?;
        let grouped = symbols.into_iter().into_group_map_by(|x| x.rpath.clone());

        let res_vec: Vec<_> = grouped
            .iter()
            .map(|(k, v)| {
                let total = v
                    .iter()
                    .map(|s| {
                        let SymbolInfo {
                            rpath,
                            full_range,
                            name,
                            kind,
                            container_name,
                            docs,
                            focus_range,
                        } = s;
                        format!(
                            "     {name}  {:?}  {:?}  {:?}",
                            kind, container_name, focus_range
                        )
                    })
                    .reduce(|acc, x1| format!("{acc}\n{x1}"))
                    .unwrap();
                format!("path:{k}\n{total}\n")
            })
            .collect();
        Ok(res_vec.join(""))
    }
}

impl RustProject {
    pub(crate) fn new(cur_dir: &PathBuf) -> Result<RustProject, anyhow::Error> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
        };

        let (db, vfs, _proc_macro_client) =
            load_workspace_at(cur_dir, &cargo_config, &load_config, &|msg| {})?;

        let anal_host = AnalysisHost::with_database(db.clone());

        let anal_host = Arc::new(Mutex::new(anal_host));

        let proj = RustProject {
            analysis_host: anal_host,
            vfs,
            files: HashMap::new(),
        };

        Ok(proj)
    }

    pub(crate) async fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self).await
    }
    fn modify_file(&self) {
        let joe = self.analysis_host.lock().unwrap();
    }
    pub async fn get_all_proj_symbols(
        &self,
        symbol_cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        symbol_cache.transaction(|db: &mut TypedCacheDb<_, _>| {
            if db.is_empty()? {
                let symboles = session.get_symboles()?;
                let impls = Self::get_all_trait_impls(&self.vfs, &symboles, &session);
                let combined = vec![symboles, impls].concat();
                let (_, errs): (Vec<_>, Vec<_>) = combined
                    .iter()
                    .map(|i| db.put(i, i))
                    .partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(combined)
            } else {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res)
            }
        })
    }

    fn get_all_trait_impls(
        vfs: &Vfs,
        symboles: &Vec<SymbolInfo>,
        session: &AnalysisSession<'_>,
    ) -> Vec<SymbolInfo> {
        let traits: Vec<_> = symboles
            .iter()
            .filter(|info| info.kind == SymbolKind::Trait)
            .cloned()
            .collect();

        traits
            .into_iter()
            .flat_map(|info| {
                info.focus_range.clone().map(|t| {
                    session
                        .go_to_impl(
                            vfs.file_id(&VfsPath::new_real_path(info.rpath)).unwrap().0,
                            TextSize::new(t.start),
                        )
                        .ok()
                })
            })
            .flatten()
            .flatten()
            .map(|info| info)
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_analytical_context() {
        let mut ctx = CurContext::new().await.expect("Failed to create CurContext");
        let context = ctx
            .get_analytical_context()
            .await
            .expect("Failed to get analytical context");
        println!("\n=== Analytical Context ===\n{}", context);
    }
}
