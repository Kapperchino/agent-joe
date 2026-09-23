use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    root: PathBuf,
    workspace: WorkspacePolicy,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("joe-knowledge-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let workspace = WorkspacePolicy::workspace(root.clone()).unwrap();
        workspace
            .write(
                Path::new("Cargo.toml"),
                "[package]\nname = 'fixture'\nversion = '0.1.0'\n",
            )
            .unwrap();
        workspace
            .write(Path::new("Cargo.lock"), "version = 4\n")
            .unwrap();
        workspace
            .write(Path::new("src/main.rs"), "fn main() {}\n")
            .unwrap();
        Self { root, workspace }
    }

    fn profile(&self) -> SemanticProfile {
        SemanticProfile {
            manifest: SourcePath::try_from("Cargo.toml".to_owned()).unwrap(),
            target: native_target(),
            features: Features::Default,
            configurations: [Configuration::Normal].into(),
            analyzer_version: ANALYZER_VERSION.into(),
        }
    }

    fn graph(&self) -> SemanticGraph {
        let capture = Capture::new(&self.workspace, &|| Ok(())).unwrap();
        SemanticGraph::try_from(GraphData {
            version: KNOWLEDGE_PROTOCOL_VERSION,
            profile: self.profile(),
            sources: capture
                .files
                .iter()
                .map(|(path, file)| {
                    SourceFile::new(path.clone(), file.text().unwrap().to_owned()).unwrap()
                })
                .collect(),
            symbols: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        })
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn knowledge_freshness_detects_edits_additions_deletions_and_modes() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let captured = Fingerprint::capture(&fixture.workspace).unwrap();
    assert!(captured.is_current(&fixture.workspace).unwrap());
    fixture
        .workspace
        .write(Path::new("target/ignored"), "cache")
        .unwrap();
    assert!(captured.is_current(&fixture.workspace).unwrap());
    fixture
        .workspace
        .write(Path::new("README.md"), "new file")
        .unwrap();
    assert!(!captured.is_current(&fixture.workspace).unwrap());
    fixture.workspace.delete(Path::new("README.md")).unwrap();
    assert!(captured.is_current(&fixture.workspace).unwrap());
    fs::set_permissions(
        fixture.root.join("src/main.rs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(!captured.is_current(&fixture.workspace).unwrap());
    fixture
        .workspace
        .write(Path::new("src/main.rs"), "fn main() { missing(); }\n")
        .unwrap();
    assert!(!captured.is_current(&fixture.workspace).unwrap());
    fixture.workspace.delete(Path::new("src/main.rs")).unwrap();
    assert!(!captured.is_current(&fixture.workspace).unwrap());
}

#[test]
fn knowledge_capture_owns_bytes_independently_of_later_edits() {
    let fixture = Fixture::new();
    let capture = Capture::new(&fixture.workspace, &|| Ok(())).unwrap();
    fixture
        .workspace
        .write(Path::new("src/main.rs"), "fn changed() {}")
        .unwrap();
    let graph =
        knowledge_indexer::load_sources(capture.sources().unwrap(), fixture.profile(), &|| Ok(()))
            .unwrap();
    assert_eq!(
        graph
            .data()
            .sources
            .iter()
            .find(|source| source.path().as_str() == "src/main.rs")
            .unwrap()
            .text(),
        "fn main() {}\n"
    );
    assert!(capture.check_graph(&graph, &fixture.profile()).is_ok());
    assert!(!capture.fingerprint.is_current(&fixture.workspace).unwrap());
}

#[test]
fn knowledge_capture_fails_closed_for_restricted_paths_and_links() {
    let fixture = Fixture::new();
    let restricted = fixture
        .workspace
        .restricted(
            &[PathBuf::from("src")],
            crate::workspace::RootAccess::ReadOnly,
        )
        .unwrap();
    assert!(Fingerprint::capture(&restricted).is_err());
    std::os::unix::fs::symlink(
        fixture.root.join("src/main.rs"),
        fixture.root.join("linked.rs"),
    )
    .unwrap();
    assert!(Fingerprint::capture(&fixture.workspace).is_err());
}

#[test]
fn knowledge_graph_requires_exact_profile_and_complete_source_coverage() {
    let fixture = Fixture::new();
    let graph = fixture.graph();
    let capture = Capture::new(&fixture.workspace, &|| Ok(())).unwrap();
    assert!(capture.check_graph(&graph, &fixture.profile()).is_ok());
    let mut missing = graph.data().clone();
    missing.sources.pop();
    assert!(
        capture
            .check_graph(
                &SemanticGraph::try_from(missing).unwrap(),
                &fixture.profile()
            )
            .is_err()
    );
    let mut extra = graph.data().clone();
    extra.sources.push(
        SourceFile::new(
            "extra.rs".to_owned().try_into().unwrap(),
            "fn extra() {}".into(),
        )
        .unwrap(),
    );
    assert!(
        capture
            .check_graph(&SemanticGraph::try_from(extra).unwrap(), &fixture.profile())
            .is_err()
    );
    let mut profile = fixture.profile();
    profile.features = Features::None;
    assert!(capture.check_graph(&graph, &profile).is_err());
    assert!(serde_json::from_str::<SourcePath>("\"../escape\"").is_err());
}

#[tokio::test]
async fn knowledge_preparation_is_read_only_without_lockfile_or_executable_access() {
    let fixture = Fixture::new();
    fixture.workspace.delete(Path::new("Cargo.lock")).unwrap();
    fixture
        .workspace
        .write(
            Path::new("build.rs"),
            "fn main() { std::fs::write(\"sentinel\", \"executed\").unwrap(); }",
        )
        .unwrap();
    let workspace = fixture
        .workspace
        .restricted(
            std::slice::from_ref(&fixture.root),
            crate::workspace::RootAccess::ReadOnly,
        )
        .unwrap();
    let scope = ExecutionScope::with_workspace(workspace);
    assert!(scope.sandbox().is_err());
    let before = Fingerprint::capture(&fixture.workspace).unwrap();
    let prepared = scope.enter(prepare(fixture.profile())).await.unwrap();
    assert_eq!(before, prepared.fingerprint);
    assert!(prepared.fingerprint.is_current(&fixture.workspace).unwrap());
    assert!(
        prepared
            .graph
            .data()
            .symbols
            .iter()
            .any(|symbol| symbol.name == "main" && symbol.state == DefinitionState::Resolved)
    );
    assert!(!fixture.root.join("target").exists());
    assert!(!fixture.root.join("sentinel").exists());
    scope.finish().await;
}

#[tokio::test]
async fn knowledge_preparation_honors_cancellation_and_requires_captured_manifest() {
    let fixture = Fixture::new();
    assert!(Capture::new(&fixture.workspace, &|| Err(anyhow::anyhow!("cancelled"))).is_err());
    let scope =
        ExecutionScope::with_workspace(WorkspacePolicy::workspace(fixture.root.clone()).unwrap());
    scope.cancel.cancel();
    assert!(scope.enter(prepare(fixture.profile())).await.is_err());
    scope.finish().await;
    fixture.workspace.delete(Path::new("Cargo.toml")).unwrap();
    let capture = Capture::new(&fixture.workspace, &|| Ok(())).unwrap();
    assert!(
        knowledge_indexer::load_sources(capture.sources().unwrap(), fixture.profile(), &|| Ok(()))
            .is_err()
    );
}
