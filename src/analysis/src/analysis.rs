use crate::rust_proj::RustProject;
pub(crate) use crate::symbol_info::SymbolInfo;
use ra_ap_ide::{
    Analysis, FilePosition, FileStructureConfig, GotoImplementationConfig, LineIndex,
    StructureNode, TextSize,
};
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
    proj: &'a RustProject,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Eq, PartialEq, Hash)]
pub struct Range {
    pub start: u32,
    pub end: u32,
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
                    exclude_locals: true,
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
                .map(|n| {
                    let line_ind = self.get_line_indecies(n.file_id)?;
                    SymbolInfo::from_nav(n, &vfs, line_ind.clone(), &self.proj.root)
                })
                .collect::<Result<Vec<_>, anyhow::Error>>()
        });
        res.transpose()
    }

    pub fn get_symboles(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let mut q = Query::new("".to_string());
        q.exclude_imports();
        let search_res = self.analysis.symbol_search(q, usize::MAX)?;
        let vfs = self.proj.vfs.lock().unwrap();
        let res: Result<Vec<_>, _> = search_res
            .into_iter()
            .map(|n| {
                let line_ind = self.get_line_indecies(n.file_id)?;
                SymbolInfo::from_nav(n, &vfs, line_ind.clone(), &self.proj.root)
            })
            .collect();
        res
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
