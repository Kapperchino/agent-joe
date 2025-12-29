use crate::analysis::AnalysisSession;
use crate::utils::Utils;
use anyhow::anyhow;
use futures::future;
use futures::future::err;
use heed::types::{SerdeJson, Str};
use heed::{Database, Env};
use ra_ap_hir::db::DefDatabase;
use ra_ap_hir::sym::{as_str, usize};
use ra_ap_ide::{
    Analysis, AnalysisHost, Cancellable, FileChange, FileStructureConfig, NavigationTarget,
    StructureNode, TextRange,
};
use ra_ap_ide_db::base_db::{RootQueryDb, SourceDatabase};
use ra_ap_ide_db::documentation::Documentation;
use ra_ap_ide_db::symbol_index::Query;
use ra_ap_ide_db::{ChangeWithProcMacros, RootDatabase, SymbolKind};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs::DirEntry;

pub struct CurContext {
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    rust_proj: RustProject,
    db_env: heed::Env,
}
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Vfs,
    pub files: HashMap<FileId, VfsPath>,
    db_env: heed::Env,
}
impl CurContext {
    pub async fn new(db_env: heed::Env) -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = CurContext::load_rust_project(&current_dir, db_env.clone())?;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            db_env,
        })
    }

    pub async fn to_string(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let session = self.rust_proj.new_analysis();
        let files: String = session
            .get_work_files()
            .into_iter()
            .map(|info| info.path.to_string())
            .fold(String::new(), |acc, s| format!("{acc}{s}").to_string());
        format!("Current Context: \ncurrent directory: {dir}\ncurrent files:\n{files}")
    }

    pub(crate) fn load_rust_project(cur_dir: &PathBuf, db_env: Env) -> Result<RustProject, anyhow::Error> {
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
            db_env,
        };

        Ok(proj)
    }
}

impl RustProject {
    pub(crate) fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self.db_env.clone(), self)
    }
    fn modify_file(&self) {
        let joe = self.analysis_host.lock().unwrap();
    }
}
