use super::*;
use crate::discovery::{SearchMode, SearchQuery, SearchTarget};

struct Fixture {
    root: PathBuf,
    workspace: WorkspacePolicy,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("joe-inventory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let workspace = WorkspacePolicy::workspace(root).unwrap();
        Self {
            root: workspace.root().to_path_buf(),
            workspace,
        }
    }
    fn write(&self, path: &str, text: &str) {
        self.workspace.write(Path::new(path), text).unwrap();
    }
    fn paths(&self) -> Vec<PathBuf> {
        Inventory::scan(&self.workspace).unwrap().files
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn inventory_covers_non_rust_empty_hidden_and_new_files_with_nested_ignore_rules() {
    let fixture = Fixture::new();
    for path in [
        "Cargo.toml",
        "README.md",
        ".github/workflows/ci.yml",
        "fixtures/case.txt",
        "src/empty.rs",
        "empty",
        "ignored.txt",
        "nested/drop.txt",
        "nested/keep.txt",
        "target/build.rs",
    ] {
        fixture.write(path, "needle");
    }
    fixture.write(".gitignore", "ignored.txt\n*.tmp\nnested/*.txt\n");
    fixture.write("nested/.gitignore", "!keep.txt\n");
    fixture.write("nested/.ignore", "*.rs\n");
    fixture.write("nested/hidden.rs", "needle");
    let files = fixture.paths();
    for path in [
        "Cargo.toml",
        "README.md",
        ".github/workflows/ci.yml",
        "fixtures/case.txt",
        "src/empty.rs",
        "empty",
        "nested/keep.txt",
    ] {
        assert!(files.contains(&PathBuf::from(path)), "{path}");
    }
    for path in [
        "ignored.txt",
        "nested/drop.txt",
        "nested/hidden.rs",
        "target/build.rs",
    ] {
        assert!(!files.contains(&PathBuf::from(path)), "{path}");
    }
    assert_eq!(
        fixture.workspace.read(Path::new("ignored.txt")).unwrap(),
        "needle"
    );
    fixture.write("new.md", "");
    assert!(fixture.paths().contains(&PathBuf::from("new.md")));
    fixture
        .workspace
        .rename(Path::new("new.md"), Path::new("renamed.md"))
        .unwrap();
    assert!(!fixture.paths().contains(&PathBuf::from("new.md")));
    fixture.workspace.delete(Path::new("renamed.md")).unwrap();
    assert!(!fixture.paths().contains(&PathBuf::from("renamed.md")));
}

#[test]
fn discovery_matches_unicode_text_and_literal_paths_with_filters_and_limits() {
    let fixture = Fixture::new();
    for path in [
        "Cargo.toml",
        "README.md",
        ".github/workflows/ci.yml",
        "fixtures/日本語.txt",
    ] {
        fixture.write(path, "first\nneedle.[終]\nlast\n");
    }
    let query =
        SearchQuery::new("needle.[終]", SearchMode::Literal, "", "", Some(10), 1, 1).unwrap();
    let result = query
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert_eq!(result.matches.len(), 4);
    assert!(result.matches.iter().all(|found| found.line == Some(2)
        && found.lines[0].line == 1
        && found.lines[2].line == 3));
    assert!(!result.truncated);
    let filtered = SearchQuery::new(
        "needle",
        SearchMode::Regex,
        "**/*.txt\n*.md",
        "README.md",
        Some(10),
        0,
        0,
    )
    .unwrap();
    let result = filtered
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].path, PathBuf::from("fixtures/日本語.txt"));
    let paths = SearchQuery::new(".github/", SearchMode::Literal, "", "", Some(1), 0, 0).unwrap();
    let result = paths
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Paths,
        )
        .unwrap();
    assert_eq!(
        result.matches[0].path,
        PathBuf::from(".github/workflows/ci.yml")
    );
    assert!(!result.truncated);
    let limited = SearchQuery::new("needle", SearchMode::Regex, "", "", Some(2), 0, 0).unwrap();
    let result = limited
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert_eq!(result.matches.len(), 2);
    assert!(result.truncated);
    assert!(SearchQuery::new("[", SearchMode::Regex, "", "", None, 0, 0).is_err());
    assert!(SearchQuery::new("", SearchMode::Literal, "[", "", None, 0, 0).is_err());
    assert!(ResultLimit::new(Some(0)).is_err());
    assert!(ResultLimit::new(Some(1001)).is_err());
}

#[test]
fn large_workspace_results_and_directory_pages_are_deterministic_and_bounded() {
    let fixture = Fixture::new();
    for index in (0..1100).rev() {
        fixture.write(&format!("files/{index:04}.txt"), "needle\nneedle\n");
    }
    let query = SearchQuery::new("needle", SearchMode::Literal, "", "", None, 0, 0).unwrap();
    let result = query
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert_eq!(result.matches.len(), 200);
    assert!(result.truncated);
    assert_eq!(result.matches[0].path, PathBuf::from("files/0000.txt"));
    let first = Listing::read(
        &fixture.workspace,
        Path::new("files"),
        0,
        ResultLimit::new(None).unwrap(),
    )
    .unwrap();
    assert_eq!(first.total, 1100);
    assert_eq!(first.entries.len(), 200);
    assert_eq!(first.next_offset, Some(200));
    let last = Listing::read(
        &fixture.workspace,
        Path::new("files"),
        1000,
        ResultLimit::new(None).unwrap(),
    )
    .unwrap();
    assert_eq!(last.entries.len(), 100);
    assert!(!last.truncated);
    assert_eq!(last.next_offset, None);
}

#[cfg(unix)]
#[test]
fn discovery_excludes_links_and_protected_storage_and_reports_binary_skips() {
    let fixture = Fixture::new();
    fixture.write("allowed.txt", "needle");
    std::os::unix::fs::symlink("allowed.txt", fixture.root.join("alias.txt")).unwrap();
    let linked = crate::test_support::permitted(
        "create hard links",
        std::fs::hard_link(
            fixture.root.join("allowed.txt"),
            fixture.root.join("hard.txt"),
        ),
    )
    .is_some();
    std::fs::create_dir_all(fixture.root.join(".turbo-code")).unwrap();
    std::fs::write(fixture.root.join(".turbo-code/private"), "secret").unwrap();
    fixture.write("binary", "needle\0");
    let expected = match linked {
        true => vec![PathBuf::from("binary")],
        false => vec![PathBuf::from("allowed.txt"), PathBuf::from("binary")],
    };
    assert_eq!(fixture.paths(), expected);
    let query = SearchQuery::new("needle", SearchMode::Literal, "", "", None, 0, 0).unwrap();
    let result = query
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert_eq!(result.matches.len(), usize::from(!linked));
    assert_eq!(result.skipped[0].reason, "Binary content");
    assert!(result.skipped_files >= if linked { 4 } else { 2 });
}

#[test]
fn search_output_limit_reports_truncation_and_retains_complete_matches() {
    let fixture = Fixture::new();
    let line = format!("needle{}\n", "x".repeat(1024 * 1024));
    fixture.write("large.txt", &line.repeat(8));
    let query = SearchQuery::new("needle", SearchMode::Literal, "", "", None, 8, 8).unwrap();
    let result = query
        .search(
            &fixture.workspace,
            Inventory::scan(&fixture.workspace).unwrap(),
            SearchTarget::Text,
        )
        .unwrap();
    assert!(result.truncated);
    assert!(matches!(
        result.truncation,
        Some(crate::discovery::TruncationReason::OutputLimit)
    ));
    assert_eq!(result.matches.len(), 3);
    assert!(result.matches.iter().all(|found| found.lines.len() == 8));
    assert!(serde_json::to_vec(&result).unwrap().len() < 32 * 1024 * 1024);
}
