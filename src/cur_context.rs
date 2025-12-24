use crate::utils::Utils;
use futures::future;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::Vfs;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs::DirEntry;

pub struct CurContext {
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    rust_proj: RustProject,
}
pub struct RustProject {
    pub db: Arc<Mutex<RootDatabase>>,
    pub vfs: Vfs,
}
impl CurContext {
    pub async fn get_cur_context() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = CurContext::load_rust_project(&current_dir)?;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
        })
    }

    pub async fn to_string(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let files: Vec<_> = self
            .cur_files
            .iter()
            .map(async |file| {
                let path = file.path();
                let file_type = match file.file_type().await {
                    Ok(f_type) => {
                        if f_type.is_dir() {
                            Some("type: dir")
                        } else if f_type.is_file() {
                            Some("type: file")
                        } else if f_type.is_symlink() {
                            Some("type: symlink")
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                if let Some(f_type) = file_type
                    && let Some(p_str) = path.to_str()
                {
                    Some(format!("path: {p_str}, {f_type}\n").to_string())
                } else {
                    None
                }
            })
            .collect();
        let res: String = future::join_all(files)
            .await
            .into_iter()
            .flatten()
            .fold(String::new(), |acc, s| format!("{acc}{s}").to_string());
        format!("Current Context: \ncurrent directory: {dir}\ncurrent files:\n{res}")
    }

    pub fn load_rust_project(cur_dir: &PathBuf) -> Result<RustProject, anyhow::Error> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: true,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: true,
        };

        let (db, vfs, _proc_macro_client) =
            load_workspace_at(cur_dir, &cargo_config, &load_config, &|msg| {})?;

        let db = Arc::new(Mutex::new(db));

        Ok(RustProject { db, vfs })
    }
}
