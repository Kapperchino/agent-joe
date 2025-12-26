use crate::utils::Utils;
use futures::future;
use ra_ap_hir::db::DefDatabase;
use ra_ap_ide::{
    Analysis, AnalysisHost, Cancellable, FileChange, FileStructureConfig, StructureNode,
};
use ra_ap_ide_db::base_db::{RootQueryDb, SourceDatabase};
use ra_ap_ide_db::{ChangeWithProcMacros, RootDatabase};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
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
}
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Vfs,
    pub files: HashMap<FileId, VfsPath>,
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

    fn load_rust_project(cur_dir: &PathBuf) -> Result<RustProject, anyhow::Error> {
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
}

pub struct FileInfo {
    pub id: FileId,
    pub path: VfsPath,
}

#[derive(Debug)]
pub struct CrateInfo {
    pub name: String,
    pub version: String,
    pub file_id: FileId,
}

pub struct AnalysisSession<'a> {
    analysis: Analysis,
    proj: &'a RustProject,
}

impl AnalysisSession<'_> {
    fn get_work_files(&self) -> Vec<FileInfo> {
        self.proj
            .vfs
            .iter()
            .filter(|(id, _path)| {
                self.analysis
                    .source_root_id(id.clone())
                    .and_then(|t| self.analysis.is_local_source_root(t))
                    .unwrap_or(false)
            })
            .map(|(id, path)| FileInfo {
                id,
                path: path.clone(),
            })
            .collect()
    }

    fn get_dependenceis(&self) -> Vec<CrateInfo> {
        self.analysis
            .fetch_crates()
            .unwrap()
            .iter()
            .map(|c| CrateInfo {
                name: c.name.clone().unwrap_or("".to_string()),
                version: c.version.clone().unwrap_or("".to_string()),
                file_id: c.root_file_id,
            })
            .collect()
    }

    fn get_syntax_tree(&self, file_id: FileId) -> String {
        self.analysis
            .view_syntax_tree(file_id)
            .unwrap_or("".to_string())
    }

    fn get_file_structure(&self, file_id: FileId) -> Vec<StructureNode> {
        self.analysis
            .file_structure(
                &FileStructureConfig {
                    exclude_locals: false,
                },
                file_id,
            )
            .unwrap_or(vec![])
    }

    fn get_item_tree(&self, file_id: FileId) -> String {
        self.analysis
            .view_item_tree(file_id)
            .unwrap_or(String::default())
    }
}

impl RustProject {
    fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession {
            analysis,
            proj: self,
        }
    }
    fn modify_file(&self) {
        let joe = self.analysis_host.lock().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_work_files() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = CurContext::load_rust_project(&cur_dir).expect("Failed to load rust project");

        let session = project.new_analysis();
        let work_files = session.get_work_files();

        println!("\n=== Work Files ({} total) ===", work_files.len());
        for file_info in &work_files {
            println!("FileId: {:?}, Path: {}", file_info.id, file_info.path);
        }
        println!("=== End Work Files ===\n");

        assert!(!work_files.is_empty(), "Expected at least one work file");
    }

    #[test]
    fn test_get_dependencies() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = CurContext::load_rust_project(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis();
        let dependencies = session.get_dependenceis();

        println!("\n=== Crate Graph ===");
        println!("{:?}", dependencies);
        println!("=== End Crate Graph ===\n");

        assert!(!dependencies.is_empty(), "Expected non-empty crate graph");
    }

    #[test]
    fn test_get_syntax_tree() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = CurContext::load_rust_project(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis();

        let work_files = session.get_work_files();
        assert!(!work_files.is_empty(), "Expected at least one work file");

        // Get syntax tree for the first file
        let actor_state = work_files
            .iter()
            .find(|x| {
                x.path.as_path().unwrap() == "/Users/kamranorhun/Dev/turbo-code/src/actor_state.rs"
            })
            .unwrap();
        let syntax_tree = session.get_syntax_tree(actor_state.id);

        println!("\n=== Syntax Tree for {} ===", actor_state.path);
        println!("{}", syntax_tree);
        println!("=== End Syntax Tree ===\n");

        assert!(!syntax_tree.is_empty(), "Expected non-empty syntax tree");
    }

    #[test]
    fn test_get_file_structure() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = CurContext::load_rust_project(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis();

        let work_files = session.get_work_files();
        assert!(!work_files.is_empty(), "Expected at least one work file");

        let actor_state = work_files
            .iter()
            .find(|x| {
                x.path.as_path().unwrap() == "/Users/kamranorhun/Dev/turbo-code/src/actor_state.rs"
            })
            .unwrap();
        let file_structure = session.get_file_structure(actor_state.id);

        println!("\n=== File Structure for {} ===", actor_state.path);
        for node in &file_structure {
            println!("{:?}", node);
        }
        println!("=== End File Structure ===\n");

        assert!(
            !file_structure.is_empty(),
            "Expected non-empty file structure"
        );
    }

    #[test]
    fn test_get_item_tree() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = CurContext::load_rust_project(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis();

        let work_files = session.get_work_files();
        assert!(!work_files.is_empty(), "Expected at least one work file");

        let actor_state = work_files
            .iter()
            .find(|x| {
                x.path.as_path().unwrap() == "/Users/kamranorhun/Dev/turbo-code/src/actor_state.rs"
            })
            .unwrap();
        let file_structure = session.get_item_tree(actor_state.id);

        println!("{}", file_structure);

        assert!(
            !file_structure.is_empty(),
            "Expected non-empty file structure"
        );
    }
}
