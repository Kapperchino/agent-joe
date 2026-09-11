use crate::analysis::{AnalysisSession, FileInfo};
use crate::symbol_info::SymbolInfo;
use ra_ap_ide::{AnalysisHost, SourceRoot};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_vfs::file_set::FileSet;
use ra_ap_vfs::{Change, FileId, Vfs, VfsPath};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use utils::workspace::WorkspacePolicy;

#[derive(Clone)]
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Arc<Mutex<Vfs>>,
    pub root: String,
    workspace: Arc<WorkspacePolicy>,
    sync_lock: Arc<Mutex<()>>,
    refresh_lock: Arc<Mutex<()>>,
}

impl RustProject {
    pub(crate) fn new(cur_dir: &Path) -> anyhow::Result<Self> {
        let workspace = Arc::new(WorkspacePolicy::workspace(cur_dir.to_path_buf())?);
        let project = Self {
            analysis_host: Arc::new(Mutex::new(AnalysisHost::default())),
            vfs: Arc::new(Mutex::new(Vfs::default())),
            root: workspace.root().to_string_lossy().into_owned(),
            workspace,
            sync_lock: Arc::new(Mutex::new(())),
            refresh_lock: Arc::new(Mutex::new(())),
        };
        project.refresh_from_disk()?;
        Ok(project)
    }

    pub fn refresh_from_disk(&self) -> anyhow::Result<bool> {
        let _refresh = self.refresh_lock.lock().unwrap();
        let files = utils::inventory::Inventory::scan(&self.workspace)?
            .files
            .into_iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .filter_map(|path| {
                self.workspace.read(&path).ok().map(|content| {
                    (
                        VfsPath::new_real_path(
                            self.workspace
                                .root()
                                .join(path)
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        content.into_bytes(),
                    )
                })
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let _sync = self.sync_lock.lock().unwrap();
        {
            let mut vfs = self.vfs.lock().unwrap();
            let removed = vfs
                .iter()
                .filter(|(_, path)| !files.contains_key(path))
                .map(|(_, path)| path.clone())
                .collect::<Vec<_>>();
            for path in removed {
                vfs.set_file_contents(path, None);
            }
            for (path, content) in files {
                vfs.set_file_contents(path, Some(content));
            }
        }
        self.apply_vfs_changes_locked()
    }

    pub fn workspace(&self) -> Arc<WorkspacePolicy> {
        self.workspace.clone()
    }

    pub fn get_file_id(&self, path: PathBuf) -> Option<FileId> {
        self.vfs
            .lock()
            .unwrap()
            .file_id(&VfsPath::new_real_path(path.to_string_lossy().into_owned()))
            .map(|entry| entry.0)
    }

    pub async fn new_analysis(&self) -> AnalysisSession<'_> {
        let _ = self.apply_vfs_changes().inspect_err(|error| {
            tracing::error!("Failed to apply pending VFS changes: {error}");
        });
        let _sync = self.sync_lock.lock().unwrap();
        let work_files = self.local_work_files();
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self, work_files, _sync)
    }

    pub fn apply_vfs_changes(&self) -> anyhow::Result<bool> {
        let _sync = self.sync_lock.lock().unwrap();
        self.apply_vfs_changes_locked()
    }

    fn apply_vfs_changes_locked(&self) -> anyhow::Result<bool> {
        let mut vfs = self.vfs.lock().unwrap();
        let changes = vfs.take_changes();
        if changes.is_empty() {
            Ok(false)
        } else {
            let mut file_set = FileSet::default();
            for (id, path) in vfs.iter() {
                file_set.insert(id, path.clone());
            }
            let mut change = ChangeWithProcMacros::default();
            change.set_roots(vec![SourceRoot::new_local(file_set)]);
            for changed in changes.into_values() {
                let content = match changed.change {
                    Change::Create(content, _) | Change::Modify(content, _) => {
                        Some(String::from_utf8(content)?)
                    }
                    Change::Delete => None,
                };
                change.change_file(changed.file_id, content);
            }
            self.analysis_host.lock().unwrap().apply_change(change);
            Ok(true)
        }
    }

    pub async fn get_all_proj_symbols(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        session
            .get_work_files()
            .into_iter()
            .try_fold(Vec::new(), |mut symbols, file| {
                let path = file
                    .path
                    .as_path()
                    .ok_or_else(|| anyhow::anyhow!("Expected a project file path"))?;
                symbols.extend(SymbolInfo::from_file_structs(
                    file.id,
                    session.get_file_structure(file.id),
                    PathBuf::from(path.as_str()),
                    session.get_line_indecies(file.id)?,
                    &self.root,
                )?);
                Ok(symbols)
            })
    }

    fn local_work_files(&self) -> Vec<FileInfo> {
        let mut files: Vec<_> = self
            .vfs
            .lock()
            .unwrap()
            .iter()
            .map(|(id, path)| FileInfo {
                id,
                path: path.clone(),
            })
            .collect();
        files.sort_by_key(|file| file.id.index());
        files
    }
}

#[cfg(test)]
#[path = "../tests/unit/rust_proj/tests.rs"]
mod tests;
