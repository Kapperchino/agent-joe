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
