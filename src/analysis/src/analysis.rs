use crate::rust_proj::RustProject;
pub(crate) use crate::symbol_info::SymbolInfo;
use ra_ap_ide::{
    Analysis, FilePosition, FileStructureConfig, GotoImplementationConfig, LineIndex,
    StructureNode, TextSize,
};
use ra_ap_ide_db::symbol_index::Query;
use ra_ap_vfs::{FileId, VfsPath};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone)]
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
    work_files: Vec<FileInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Eq, PartialEq, Hash)]
pub struct Range {
    pub start: u32,
    pub end: u32,
}

impl<'a> AnalysisSession<'a> {
    pub(crate) fn get_work_files(&self) -> Vec<FileInfo> {
        self.work_files.clone()
    }
    
    pub fn get_file_id(&self, path: &Path) -> Option<FileId> {
        let path = VfsPath::new_real_path(path.to_string_lossy().to_string());
        self.work_files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.id)
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

    pub fn new(analysis: Analysis, proj: &'a RustProject, work_files: Vec<FileInfo>) -> Self {
        Self {
            analysis,
            proj,
            work_files,
        }
    }
}
