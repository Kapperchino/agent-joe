use crate::analysis::{AnalysisSession, FileInfo};
use crate::symbol_info::SymbolInfo;
use dashmap::DashMap;
use ra_ap_ide::SourceRoot;
use ra_ap_ide::{AnalysisHost, TextSize};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_ide_db::SymbolKind;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::file_set::FileSet;
use ra_ap_vfs::{Change, FileId, Vfs, VfsPath};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::error;

#[derive(Clone, Debug, Default)]
struct TrackedSourceRoot {
    is_library: bool,
    files: HashMap<FileId, TrackedFile>,
}

#[derive(Clone, Debug)]
struct TrackedFile {
    path: VfsPath,
    exists: bool,
}

struct PendingVfsChange {
    file_id: FileId,
    change: Change,
}

#[derive(Clone)]
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Arc<Mutex<Vfs>>,
    pub root: String,
    source_roots: Arc<DashMap<usize, TrackedSourceRoot>>,
    sync_lock: Arc<Mutex<()>>,
}
impl RustProject {
    pub(crate) fn new(cur_dir: &PathBuf) -> Result<RustProject, anyhow::Error> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
            num_worker_threads: 1,
            proc_macro_processes: 1,
        };

        let (db, vfs, _proc_macro_client) =
            load_workspace_at(cur_dir, &cargo_config, &load_config, &|msg| {})?;

        let anal_host = AnalysisHost::with_database(db.clone());

        let source_roots = Self::init_source_roots(&anal_host, &vfs)
            .into_iter()
            .enumerate()
            .collect();

        let anal_host = Arc::new(Mutex::new(anal_host));

        let proj = RustProject {
            analysis_host: anal_host,
            vfs: Arc::new(Mutex::new(vfs)),
            root: cur_dir.to_string_lossy().to_string(),
            source_roots: Arc::new(source_roots),
            sync_lock: Arc::new(Mutex::new(())),
        };

        Ok(proj)
    }

    pub fn get_file_id(&self, path: PathBuf) -> Option<FileId> {
        self.vfs
            .lock()
            .unwrap()
            .file_id(&VfsPath::new_real_path(path.to_string_lossy().to_string()))
            .map(|t| t.0)
    }

    pub async fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let _ = self.apply_vfs_changes().inspect_err(|err| {
            error!("failed to apply pending VFS changes: {err:?}");
        });
        let _sync = self.sync_lock.lock().unwrap();
        let work_files = self.local_work_files();
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self, work_files)
    }

    pub fn apply_vfs_changes(&self) -> anyhow::Result<bool> {
        let _sync = self.sync_lock.lock().unwrap();
        let changes = self.take_pending_vfs_changes();
        if changes.is_empty() {
            Ok(false)
        } else {
            let (roots, tracked_changes) = {
                let mut source_roots = self.source_roots.clone();
                let tracked_changes: HashSet<_> = changes
                    .iter()
                    .filter_map(|change| {
                        let exists =
                            matches!(change.change, Change::Create(_, _) | Change::Modify(_, _));
                        Self::set_tracked_file_exists(&mut source_roots, change.file_id, exists)
                            .then_some(change.file_id)
                    })
                    .collect();

                (Self::build_source_roots(&source_roots), tracked_changes)
            };

            let mut change = ChangeWithProcMacros::default();
            change.set_roots(roots);
            changes
                .into_iter()
                .filter(|pending| tracked_changes.contains(&pending.file_id))
                .for_each(|pending| match pending.change {
                    Change::Create(contents, _) | Change::Modify(contents, _) => {
                        change.change_file(pending.file_id, String::from_utf8(contents).ok());
                    }
                    Change::Delete => {
                        change.change_file(pending.file_id, None);
                    }
                });
            self.analysis_host.lock().unwrap().apply_change(change);
            Ok(true)
        }
    }

    pub async fn get_all_proj_symbols(&self) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        let symboles = session.get_symboles()?;
        let combined = vec![symboles].concat();
        Ok(combined)
    }

    fn get_all_trait_impls(
        vfs: Arc<Mutex<Vfs>>,
        symboles: &Vec<SymbolInfo>,
        session: &AnalysisSession<'_>,
    ) -> Vec<SymbolInfo> {
        let traits: Vec<_> = symboles
            .iter()
            .filter(|info| info.kind == SymbolKind::Trait)
            .cloned()
            .collect();

        traits
            .into_iter()
            .flat_map(|info| {
                info.focus_range.clone().map(|t| {
                    let id = vfs
                        .lock()
                        .unwrap()
                        .file_id(&VfsPath::new_real_path(info.rpath.inner))
                        .unwrap()
                        .0;
                    session.go_to_impl(id, TextSize::new(t.start)).ok()
                })
            })
            .flatten()
            .flat_map(|info| info)
            .flatten()
            .collect()
    }

    fn init_source_roots(analysis_host: &AnalysisHost, vfs: &Vfs) -> Vec<TrackedSourceRoot> {
        let analysis = analysis_host.analysis();
        let mut roots = Vec::new();

        vfs.iter()
            .filter_map(|(file_id, path)| {
                if let Ok(source_root_id) = analysis.source_root_id(file_id) {
                    Some((file_id, path, source_root_id))
                } else {
                    None
                }
            })
            .for_each(|(file_id, path, source_root_id)| {
                let idx = source_root_id.0 as usize;
                if roots.len() <= idx {
                    roots.resize_with(idx + 1, TrackedSourceRoot::default);
                }

                let is_local = analysis
                    .is_local_source_root(source_root_id)
                    .unwrap_or(false);
                roots[idx].is_library = !is_local;
                roots[idx].files.insert(
                    file_id,
                    TrackedFile {
                        path: path.clone(),
                        exists: true,
                    },
                );
            });

        roots
    }

    fn take_pending_vfs_changes(&self) -> Vec<PendingVfsChange> {
        let mut vfs = self.vfs.lock().unwrap();
        vfs.take_changes()
            .into_iter()
            .map(|(_, changed)| PendingVfsChange {
                file_id: changed.file_id,
                change: changed.change,
            })
            .collect()
    }

    fn local_work_files(&self) -> Vec<FileInfo> {
        let mut files: Vec<_> = self
            .source_roots
            .iter()
            .filter(|root| !root.is_library)
            .flat_map(|root| {
                root.files
                    .iter()
                    .filter(|(_, file)| {
                        file.exists
                            && file.path.as_path().is_some_and(|path| {
                                let path: &Path = path.as_ref();
                                path.starts_with(&self.root)
                            })
                    })
                    .map(|(id, file)| FileInfo {
                        id: *id,
                        path: file.path.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        files.sort_by_key(|file: &FileInfo| file.id.index());
        files
    }

    fn set_tracked_file_exists(
        source_roots: &mut Arc<DashMap<usize, TrackedSourceRoot>>,
        file_id: FileId,
        exists: bool,
    ) -> bool {
        source_roots
            .iter_mut()
            .any(|mut root| match root.files.get_mut(&file_id) {
                Some(file) => {
                    file.exists = exists;
                    true
                }
                None => false,
            })
    }

    fn build_source_roots(
        source_roots: &Arc<DashMap<usize, TrackedSourceRoot>>,
    ) -> Vec<SourceRoot> {
        source_roots
            .iter()
            .map(|root| {
                let mut file_set = FileSet::default();
                root.files
                    .iter()
                    .filter(|(_, file)| file.exists)
                    .for_each(|(file_id, file)| file_set.insert(*file_id, file.path.clone()));

                if root.is_library {
                    SourceRoot::new_library(file_set)
                } else {
                    SourceRoot::new_local(file_set)
                }
            })
            .collect()
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
}
