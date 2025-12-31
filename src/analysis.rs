use crate::cache::TypedCache;
use crate::cur_context::RustProject;
use anyhow::anyhow;
use ra_ap_ide::{Analysis, FileStructureConfig, StructureNode};
use ra_ap_ide_db::symbol_index::Query;
use ra_ap_ide_db::SymbolKind;
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
    symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
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

    pub fn get_symboles(&mut self) -> anyhow::Result<Vec<SymbolInfo>> {
        self.symbol_cache.transaction(|db| {
            if db.is_empty()? {
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
                let (_, errs): (Vec<_>, Vec<_>) =
                    res.iter().map(|i| db.put(i, i)).partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(res)
            } else {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res)
            }
        })
    }

    pub async fn new(analysis: Analysis, proj: &'a RustProject) -> Self {
        Self {
            analysis,
            proj,
            symbol_cache: TypedCache::new(None).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ra_ap_ide_db::SymbolKind::Function;
    use std::env;
    use std::io::SeekFrom;
    use tokio::fs::File;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    #[tokio::test]
    async fn test_get_dependencies() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = RustProject::new(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis().await;
        let dependencies = session.get_dependenceis();

        println!("\n=== Crate Graph ===");
        println!("{:?}", dependencies);
        println!("=== End Crate Graph ===\n");

        assert!(!dependencies.is_empty(), "Expected non-empty crate graph");
    }

    #[tokio::test]
    async fn test_get_file_structure() {
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = RustProject::new(&cur_dir).expect("Failed to load rust project");
        let session = project.new_analysis().await;

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
        let cur_dir = env::current_dir().expect("Failed to get current directory");
        let project = RustProject::new(&cur_dir).expect("Failed to load rust project");
        let mut session = project.new_analysis().await;

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
