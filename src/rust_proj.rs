use crate::analysis::{AnalysisSession, SymbolInfo};
use crate::cache::{TypedCache, TypedCacheDb};
use anyhow::anyhow;
use ra_ap_ide::{AnalysisHost, TextSize};
use ra_ap_ide_db::SymbolKind;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Arc<Mutex<Vfs>>,
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
            vfs: Arc::new(Mutex::new(vfs)),
        };

        Ok(proj)
    }

    pub fn get_file_id(&self, path: PathBuf) -> Option<FileId> {
        self.vfs
            .lock()
            .unwrap()
            .file_id(&VfsPath::new_real_path(path.to_string_lossy().to_string()))
            .map(|t| t.0)
    }

    pub(crate) async fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self).await
    }

    pub async fn get_all_proj_symbols(
        &self,
        symbol_cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        symbol_cache.transaction(|db: &mut TypedCacheDb<_, _>| {
            if db.is_empty()? {
                let symboles = session.get_symboles()?;
                let impls = {
                    let vfs = self.vfs.lock().unwrap();
                    Self::get_all_trait_impls(&vfs, &symboles, &session)
                };
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
            .flat_map(|info| info)
            .flatten()
            .collect()
    }
}
