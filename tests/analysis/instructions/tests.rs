use super::*;

struct Fixture {
    root: PathBuf,
    workspace: Arc<WorkspacePolicy>,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "joe-instructions-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let workspace = Arc::new(WorkspacePolicy::workspace(root).unwrap());
        Self {
            root: workspace.root().to_path_buf(),
            workspace,
        }
    }
    fn write(&self, path: &str, content: &str) {
        self.workspace.write(Path::new(path), content).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn instructions_have_ordered_provenance_and_require_delivery_before_edits() {
    let fixture = Fixture::new();
    fixture.write("global.md", "global rule: use red");
    fixture.write("AGENTS.md", "repository rule: use blue");
    fixture.write("src/AGENTS.md", "source rule: use green");
    fixture.write("src/nested/AGENTS.md", "nested rule: use purple");
    fixture.write("other/AGENTS.md", "unrelated rule");
    fixture.write(
        "src/nested/file.md",
        "file text is not instruction guidance",
    );
    let guidance = Instructions::new(fixture.workspace.clone())
        .with_global(fixture.root.join("global.md"))
        .unwrap();
    assert_eq!(guidance.sources().unwrap().len(), 2);
    let initial = guidance.operating("Built-in policy").unwrap();
    assert!(!initial.contains("nested rule"));
    assert!(
        guidance
            .prepare_edit(&["src/nested/new.md".into()])
            .is_err()
    );
    let sources = guidance.sources().unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>(),
        vec![
            fixture.root.join("global.md"),
            PathBuf::from("AGENTS.md"),
            PathBuf::from("src/AGENTS.md"),
            PathBuf::from("src/nested/AGENTS.md")
        ]
    );
    assert_eq!(sources[3].scope, "src/nested/ and descendants");
    let operating = guidance.operating("Built-in policy").unwrap();
    assert!(
        operating.find("global rule: use red").unwrap()
            < operating.find("repository rule: use blue").unwrap()
    );
    assert!(
        operating.find("repository rule: use blue").unwrap()
            < operating.find("nested rule: use purple").unwrap()
    );
    assert!(!operating.contains("unrelated rule"));
    assert!(!operating.contains("file text is not"));
    guidance
        .prepare_edit(&["src/nested/new.md".into()])
        .unwrap();
    fixture.write("src/AGENTS.md", "changed source rule");
    assert!(
        guidance
            .prepare_edit(&["src/nested/new.md".into()])
            .is_err()
    );
    assert!(
        guidance
            .operating("Built-in policy")
            .unwrap()
            .contains("changed source rule")
    );
    guidance
        .prepare_edit(&["src/nested/new.md".into()])
        .unwrap();
    assert_eq!(guidance.reset().sources().unwrap().len(), 2);
    let worker = guidance.fork();
    assert!(worker.prepare_edit(&["src/nested/new.md".into()]).is_err());
    worker.operating("Worker policy").unwrap();
    worker.prepare_edit(&["src/nested/new.md".into()]).unwrap();
}

#[test]
fn ignored_rules_apply_and_oversized_sources_fail_without_truncation() {
    let fixture = Fixture::new();
    fixture.write(".gitignore", "ignored/\nAGENTS.md\n");
    fixture.write("ignored/AGENTS.md", "ignored scoped guidance");
    let guidance = Instructions::new(fixture.workspace.clone());
    guidance.discover(&["ignored/file.txt".into()]).unwrap();
    assert!(
        guidance
            .operating("")
            .unwrap()
            .contains("ignored scoped guidance")
    );
    fixture.write("AGENTS.md", &"x".repeat(MAX_INSTRUCTION_BYTES + 1));
    let error = guidance.operating("").unwrap_err().to_string();
    assert!(error.contains("not truncated"));
    assert!(guidance.prepare_edit(&["ignored/file.txt".into()]).is_err());
    assert!(guidance.discover(&["../outside/file".into()]).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_root_uses_canonical_scope_and_nested_instruction_links_fail_closed() {
    let fixture = Fixture::new();
    fixture.write("project/AGENTS.md", "project rule");
    fixture.write("project/src/AGENTS.md", "nested rule");
    fixture.write("outside/AGENTS.md", "outside rule");
    let alias = fixture.root.join("alias");
    std::os::unix::fs::symlink(fixture.root.join("project"), &alias).unwrap();
    let workspace = Arc::new(WorkspacePolicy::workspace(alias.clone()).unwrap());
    let guidance = Instructions::new(workspace);
    guidance.discover(&[alias.join("src/new.rs")]).unwrap();
    assert_eq!(
        guidance.sources().unwrap()[1].path,
        PathBuf::from("src/AGENTS.md")
    );
    assert!(!guidance.operating("").unwrap().contains("outside rule"));
    std::fs::remove_file(fixture.root.join("project/src/AGENTS.md")).unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("outside/AGENTS.md"),
        fixture.root.join("project/src/AGENTS.md"),
    )
    .unwrap();
    assert!(guidance.operating("").is_err());
    assert!(guidance.prepare_edit(&["src/new.rs".into()]).is_err());
}

#[test]
fn active_instruction_budget_includes_metadata_and_global_changes_refresh() {
    let fixture = Fixture::new();
    fixture.write("global.md", "original global guidance");
    let guidance = Instructions::new(fixture.workspace.clone())
        .with_global(fixture.root.join("global.md"))
        .unwrap();
    guidance.operating("").unwrap();
    fixture.write("global.md", "updated global guidance");
    assert!(guidance.prepare_edit(&["new.md".into()]).is_err());
    assert!(
        guidance
            .operating("")
            .unwrap()
            .contains("updated global guidance")
    );
    for path in [
        "AGENTS.md",
        "one/AGENTS.md",
        "one/two/AGENTS.md",
        "one/two/three/AGENTS.md",
    ] {
        fixture.write(path, &"x".repeat(MAX_INSTRUCTION_BYTES));
    }
    let error = guidance
        .discover(&["one/two/three/new.md".into()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("256 KiB"));
}
