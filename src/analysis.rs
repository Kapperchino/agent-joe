use crate::cur_context::RustProject;
use anyhow::anyhow;
use heed::types::{SerdeJson, Str};
use heed::{Database, Env};
use ra_ap_ide::{Analysis, FileStructureConfig, StructureNode};
use ra_ap_ide_db::SymbolKind;
use ra_ap_ide_db::symbol_index::Query;
use ra_ap_vfs::{FileId, VfsPath};
use serde::{Deserialize, Serialize};

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

impl<'a> AnalysisSession<'a> {
    pub(crate) fn get_work_files(&self) -> Vec<FileInfo> {
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

    pub fn new(analysis: Analysis, db_env: Env, proj: &'a RustProject) -> Self {
        Self { analysis, db_env, proj }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cur_context::CurContext;
    use heed::EnvOpenOptions;
    use ra_ap_ide_db::SymbolKind::Function;
    use std::env;
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
        let session = project.new_analysis();
        let dependencies = session.get_dependenceis();

        println!("\n=== Crate Graph ===");
        println!("{:?}", dependencies);
        println!("=== End Crate Graph ===\n");

        assert!(!dependencies.is_empty(), "Expected non-empty crate graph");
    }

    #[test]
    fn test_get_file_structure() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let env = unsafe { EnvOpenOptions::new().open(&"~/.turbo-code/") }.unwrap();
        let project = CurContext::load_rust_project(&cur_dir, env.clone())
            .expect("Failed to load rust project");
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
        let session = project.new_analysis();

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
