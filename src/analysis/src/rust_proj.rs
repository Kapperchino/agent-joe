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
}

impl RustProject {
    pub(crate) fn new(cur_dir: &Path) -> anyhow::Result<Self> {
        let workspace = Arc::new(WorkspacePolicy::workspace(cur_dir.to_path_buf())?);
        let mut vfs = Vfs::default();
        let mut directories = vec![workspace.root().to_path_buf()];
        while let Some(directory) = directories.pop() {
            for entry in workspace.entries(&directory)? {
                if ![".git", ".agents", ".codex", ".turbo-code", "target"]
                    .iter()
                    .any(|name| entry.name.eq_ignore_ascii_case(name))
                {
                    match workspace.is_directory(&entry.path) {
                        Ok(true) => directories.push(entry.path),
                        Ok(false)
                            if entry
                                .path
                                .extension()
                                .is_some_and(|extension| extension == "rs") =>
                        {
                            let content = workspace.read(&entry.path)?;
                            vfs.set_file_contents(
                                VfsPath::new_real_path(entry.path.to_string_lossy().into_owned()),
                                Some(content.into_bytes()),
                            );
                        }
                        Ok(false) | Err(_) => {}
                    }
                }
            }
        }
        let project = Self {
            analysis_host: Arc::new(Mutex::new(AnalysisHost::default())),
            vfs: Arc::new(Mutex::new(vfs)),
            root: workspace.root().to_string_lossy().into_owned(),
            workspace,
            sync_lock: Arc::new(Mutex::new(())),
        };
        project.apply_vfs_changes()?;
        Ok(project)
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
        AnalysisSession::new(analysis, self, work_files)
    }

    pub fn apply_vfs_changes(&self) -> anyhow::Result<bool> {
        let _sync = self.sync_lock.lock().unwrap();
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
mod tests {
    fn workspace_scope() -> utils::execution::ExecutionScope {
        utils::execution::ExecutionScope::with_workspace(
            utils::workspace::WorkspacePolicy::workspace(std::env::temp_dir()).unwrap(),
        )
    }

    use super::*;
    use crate::proj_meta::ProjMeta;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct ProjectFixture {
        directory: PathBuf,
    }

    impl ProjectFixture {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "joe-analysis-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(directory.join("app/src")).unwrap();
            fs::create_dir_all(directory.join("dependency/src")).unwrap();
            fs::write(
                directory.join("app/Cargo.toml"),
                "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n\
                 [dependencies]\ndependency = { path = '../dependency' }\n",
            )
            .unwrap();
            fs::write(
                directory.join("app/src/lib.rs"),
                "pub struct Local;\npub fn local() { let _ = dependency::External; }\n",
            )
            .unwrap();
            fs::write(
                directory.join("dependency/Cargo.toml"),
                "[package]\nname = 'dependency'\nversion = '0.1.0'\nedition = '2024'\n",
            )
            .unwrap();
            fs::write(
                directory.join("dependency/src/lib.rs"),
                "pub struct External;\n",
            )
            .unwrap();
            Self {
                directory: directory.canonicalize().unwrap(),
            }
        }
    }

    impl Drop for ProjectFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    #[tokio::test]
    async fn project_index_excludes_external_dependencies() {
        workspace_scope()
            .enter(async {
                let fixture = ProjectFixture::new();
                let root = fixture.directory.join("app");
                let project = RustProject::new(&root).unwrap();
                let symbols = project.get_all_proj_symbols().await.unwrap();

                assert!(symbols.iter().any(|symbol| symbol.name == "Local"));
                assert!(symbols.iter().any(|symbol| symbol.name == "local"));
                assert!(
                    symbols
                        .iter()
                        .all(|symbol| symbol.rpath.inner == "src/lib.rs")
                );

                let hashes = ProjMeta::get_file_hashes(&project).await.unwrap();
                assert!(
                    hashes
                        .iter()
                        .any(|(path, _)| path == &root.join("src/lib.rs"))
                );
                assert!(hashes.iter().all(|(path, _)| path.starts_with(&root)));

                let metadata = ProjMeta::get_proj_meta_from_symbols(symbols, &project)
                    .await
                    .unwrap();
                assert!(
                    metadata
                        .files
                        .values()
                        .any(|file| file.rpath == "src/lib.rs")
                );
                assert!(
                    metadata
                        .files
                        .keys()
                        .all(|path| Path::new(path).is_relative())
                );
            })
            .await;
    }

    #[tokio::test]
    async fn project_index_loads_from_a_subdirectory() {
        workspace_scope()
            .enter(async {
                let fixture = ProjectFixture::new();
                let root = fixture.directory.join("app/src");
                let project = RustProject::new(&root).unwrap();
                let symbols = project.get_all_proj_symbols().await.unwrap();

                assert!(symbols.iter().any(|symbol| symbol.name == "Local"));
                assert!(symbols.iter().all(|symbol| symbol.rpath.inner == "lib.rs"));

                let hashes = ProjMeta::get_file_hashes(&project).await.unwrap();
                assert_eq!(hashes.len(), 1);
                assert_eq!(hashes[0].0, root.join("lib.rs"));
            })
            .await;
    }
    #[tokio::test]
    async fn startup_does_not_execute_project_helpers_or_follow_external_links() {
        let fixture = ProjectFixture::new();
        let root = fixture.directory.join("app");
        let marker = fixture.directory.join("executed");
        fs::write(
            root.join("build.rs"),
            format!("fn main() {{ std::fs::write({marker:?}, \"escaped\").unwrap(); }}"),
        )
        .unwrap();
        fs::create_dir(root.join(".cargo")).unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            "[build]\nrustc-wrapper = '/does-not-exist'\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            fixture.directory.join("dependency/src/lib.rs"),
            root.join("src/external.rs"),
        )
        .unwrap();
        let project = RustProject::new(&root).unwrap();
        let symbols = project.get_all_proj_symbols().await.unwrap();
        assert!(symbols.iter().any(|symbol| symbol.name == "Local"));
        assert!(symbols.iter().all(|symbol| symbol.name != "External"));
        assert!(!marker.exists());
    }
}
