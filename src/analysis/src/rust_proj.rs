use crate::analysis::AnalysisSession;
use crate::symbol_info::SymbolInfo;
use ra_ap_ide::{AnalysisHost, TextSize};
use ra_ap_ide_db::SymbolKind;
use ra_ap_load_cargo::{load_workspace_at, LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Arc<Mutex<Vfs>>,
    pub root: String,
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
            root: cur_dir.to_string_lossy().to_string(),
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

    pub async fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self).await
    }

    pub async fn get_all_proj_symbols(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        let symboles = session.get_symboles()?;
        let combined = vec![symboles].concat();
        Ok(combined)
    }

    fn get_all_trait_impls(
        vfs: Arc<Mutex<Vfs>>,
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
                    let id = vfs
                        .lock()
                        .unwrap()
                        .file_id(&VfsPath::new_real_path(info.rpath))
                        .unwrap()
                        .0;
                    session.go_to_impl(id, TextSize::new(t.start)).ok()
                })
            })
            .flatten()
            .flat_map(|info| info)
            .flatten()
            .collect()
    }
}
