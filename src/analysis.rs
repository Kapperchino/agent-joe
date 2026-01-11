use crate::cur_context::RustProject;
use ra_ap_ide::{
    Analysis, FilePosition, FileStructureConfig, GotoImplementationConfig, LineIndex,
    NavigationTarget, StructureNode, TextSize,
};
use ra_ap_ide_db::symbol_index::Query;
use ra_ap_ide_db::SymbolKind;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
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
    proj: &'a RustProject,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Range {
    pub(crate) start: u32,
    pub(crate) end: u32,
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SymbolInfo {
    pub rpath: String,
    pub full_range: Range,
    pub focus_range: Option<Range>,
    pub name: String,
    #[serde(with = "SymbolKindDef")]
    pub kind: SymbolKind,
    pub container_name: Option<String>,
    pub docs: Option<String>,
}

impl SymbolInfo {
    fn from_nav(n: NavigationTarget, vfs: &Vfs) -> Self {
        SymbolInfo {
            rpath: vfs.file_path(n.file_id).to_string(),
            full_range: Range {
                start: n.full_range.start().into(),
                end: n.full_range.end().into(),
            },
            focus_range: n.focus_range.map(|t| Range {
                start: t.start().into(),
                end: t.end().into(),
            }),
            name: n.name.to_string(),
            kind: n.kind.unwrap(),
            container_name: n.container_name.map(|s| s.to_string()),
            docs: n.docs.map(|d| d.as_str().to_string()),
        }
    }
}

impl<'a> AnalysisSession<'a> {
    pub(crate) fn get_work_files(&self) -> Vec<FileInfo> {
        let vfs = self.proj.vfs.lock().unwrap();
        vfs.iter()
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

    pub fn get_dependenceis(&self) -> Vec<CrateInfo> {
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

    pub fn get_syntax_tree(&self, file_id: FileId) -> String {
        self.analysis
            .view_syntax_tree(file_id)
            .unwrap_or("".to_string())
    }

    pub fn get_file_structure(&self, file_id: FileId) -> Vec<StructureNode> {
        self.analysis
            .file_structure(
                &FileStructureConfig {
                    exclude_locals: false,
                },
                file_id,
            )
            .unwrap_or(vec![])
    }

    pub fn go_to_impl(
        &self,
        file_id: FileId,
        offset: TextSize,
    ) -> anyhow::Result<Option<Vec<SymbolInfo>>> {
        let res = self.analysis.goto_implementation(
            &GotoImplementationConfig {
                filter_adjacent_derive_implementations: false,
            },
            FilePosition { file_id, offset },
        )?;
        let vfs = self.proj.vfs.lock().unwrap();
        let res = res.map(|rinfo| {
            rinfo
                .info
                .into_iter()
                .map(|n| SymbolInfo::from_nav(n, &vfs))
                .collect()
        });
        Ok(res)
    }

    pub fn get_symboles(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let mut q = Query::new("".to_string());
        q.exclude_imports();
        let search_res = self.analysis.symbol_search(q, usize::MAX)?;
        let vfs = self.proj.vfs.lock().unwrap();
        let res: Vec<_> = search_res
            .into_iter()
            .map(|n| SymbolInfo::from_nav(n, &vfs))
            .collect();
        Ok(res)
    }

    pub fn get_line_indecies(&self, file: FileId) -> anyhow::Result<triomphe::Arc<LineIndex>> {
        Ok(self.analysis.file_line_index(file)?)
    }

    pub async fn new(analysis: Analysis, proj: &'a RustProject) -> Self {
        Self { analysis, proj }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

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
}
