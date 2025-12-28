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

    fn load_rust_project(cur_dir: &PathBuf, db_env: Env) -> Result<RustProject, anyhow::Error> {
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
    db_env: Env,
    proj: &'a RustProject,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Range {
    start: u32,
    end: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "SymbolKind")]
enum SymbolKindDef {
    Attribute,
    BuiltinAttr,
    Const,
    ConstParam,
    Derive,
    DeriveHelper,
    Enum,
    Field,
    Function,
    Method,
    Impl,
    InlineAsmRegOrRegClass,
    Label,
    LifetimeParam,
    Local,
    Macro,
    ProcMacro,
    Module,
    SelfParam,
    SelfType,
    Static,
    Struct,
    ToolModule,
    Trait,
    TypeAlias,
    TypeParam,
    Union,
    ValueParam,
    Variant,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SymbolInfo {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    #[serde(with = "SymbolKindDef")]
    pub kind: SymbolKind,
    pub container_name: Option<String>,
    pub docs: Option<String>,
}

impl SymbolInfo {
    pub fn get_key(&self) -> String {
        let SymbolInfo {
            rpath,
            name,
            kind,
            container_name,
            ..
        } = self;
        let container_name = container_name.clone().unwrap_or("self".to_string());
        format!("{rpath}-{container_name}-{:?}-{name}", kind)
    }
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

    fn get_symboles(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let mut wtxn = self.db_env.write_txn()?;
        let db: Database<Str, SerdeJson<SymbolInfo>> =
            self.db_env.create_database(&mut wtxn, None)?;
        if db.is_empty(&wtxn)? {
            let mut q = Query::new("".to_string());
            q.exclude_imports();
            let search_res = self.analysis.symbol_search(q, usize::MAX)?;
            let res: Vec<SymbolInfo> = search_res
                .into_iter()
                .map(|n| SymbolInfo {
                    rpath: self.proj.vfs.file_path(n.file_id).to_string(),
                    full_range: Range {
                        start: n.full_range.start().into(),
                        end: n.full_range.end().into(),
                    },
                    name: n.name.to_string(),
                    kind: n.kind.unwrap(),
                    container_name: n.container_name.map(|s| s.to_string()),
                    docs: n.docs.map(|d| d.as_str().to_string()),
                })
                .collect();
            let (_, errs): (Vec<_>, Vec<_>) = res
                .iter()
                .map(|i| db.put(&mut wtxn, i.get_key().as_str(), &i))
                .partition(|r| r.is_ok());
            // report on this
            let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
            println!("{:?}", errs);
            if !errs.is_empty() {
                return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
            }
            wtxn.commit()?;
            Ok(res)
        } else {
            let res: Vec<_> = db.iter(&wtxn)?.flat_map(|x| x.ok().map(|t| t.1)).collect();
            Ok(res)
        }
    }
}

impl RustProject {
    fn new_analysis(&'_ self, db_env: Env) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession {
            analysis,
            proj: self,
            db_env,
        }
    }
    fn modify_file(&self) {
        let joe = self.analysis_host.lock().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heed::EnvOpenOptions;
    use ra_ap_ide_db::SymbolKind::Function;
    use std::io::{ErrorKind, SeekFrom};
    use tokio::fs;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    #[test]
    fn test_get_dependencies() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let env = unsafe {
            EnvOpenOptions::new() // 100 MiB
                .open(&"~/.turbo-code/")
        }
        .unwrap();
        let project = CurContext::load_rust_project(&cur_dir, env.clone())
            .expect("Failed to load rust project");
        let session = project.new_analysis(env);
        let dependencies = session.get_dependenceis();

        println!("\n=== Crate Graph ===");
        println!("{:?}", dependencies);
        println!("=== End Crate Graph ===\n");

        assert!(!dependencies.is_empty(), "Expected non-empty crate graph");
    }

    #[test]
    fn test_get_file_structure() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let env = unsafe {
            EnvOpenOptions::new() // 100 MiB
                .open(&"~/.turbo-code/")
        }
        .unwrap();
        let project = CurContext::load_rust_project(&cur_dir, env.clone())
            .expect("Failed to load rust project");
        let session = project.new_analysis(env);

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

    #[tokio::test]
    async fn test_get_item_tree() {
        match fs::create_dir("/Users/kamranorhun/.turbo-code/").await {
            Ok(_) => Ok(()),
            Err(err) => {
                if err.kind() != ErrorKind::AlreadyExists {
                    Err(err)
                } else {
                    Ok(())
                }
            }
        }
        .unwrap();
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let env = unsafe {
            EnvOpenOptions::new() // 100 MiB
                .open(&"/Users/kamranorhun/.turbo-code/")
        }
        .unwrap();
        let project = CurContext::load_rust_project(&cur_dir, env.clone())
            .expect("Failed to load rust project");
        let session = project.new_analysis(env);

        let file_structure = session.get_symboles().unwrap();

        for nav in file_structure {
            if let Function | SymbolKind::Struct = nav.kind {
                let mut file = File::open(nav.rpath.clone()).await.unwrap();
                file.seek(SeekFrom::Start((u32::from(nav.full_range.start) as u64)))
                    .await
                    .unwrap();
                let mut contents =
                    vec![0u8; u32::from(nav.full_range.end - nav.full_range.start) as usize];
                file.read_exact(&mut contents).await.unwrap();
                println!("name {:?},printout {:?}", nav, String::from_utf8(contents));
            }
        }
    }
}
