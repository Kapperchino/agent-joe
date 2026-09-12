use super::*;

struct Fixture {
    directory: PathBuf,
    policy: WorkspacePolicy,
    object: PathBuf,
    cached: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-process-{}", uuid::Uuid::new_v4()));
        let root = directory.join("workspace");
        let object = root.join("target/debug/deps/libapp.rmeta");
        let cached = root.join("target/debug/incremental/session/metadata.rmeta");
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&object, "metadata").unwrap();
        Self {
            directory,
            policy: WorkspacePolicy::workspace(root).unwrap(),
            object,
            cached,
        }
    }

    fn scan_during(&self, change: impl FnOnce()) -> WorkspaceScan {
        let mut scan = WorkspaceScan {
            links: HashMap::new(),
            directories: Vec::new(),
        };
        scan.read(&self.policy, self.object.parent().unwrap())
            .unwrap();
        change();
        scan.read(&self.policy, self.cached.parent().unwrap())
            .unwrap();
        assert!(scan.incomplete_links().is_some());
        scan
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn hard_links_created_during_a_scan_are_recounted() {
    let fixture = Fixture::new();
    let scan = fixture.scan_during(|| {
        std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
    });
    let workspace = ProcessWorkspace::from_scan(&fixture.policy, scan).unwrap();
    assert_eq!(workspace.policy().root(), fixture.policy.root());
}

#[test]
fn hard_links_removed_during_a_scan_are_recounted() {
    for remove_observed in [true, false] {
        let fixture = Fixture::new();
        std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
        let scan = fixture.scan_during(|| {
            let removed = match remove_observed {
                true => &fixture.object,
                false => &fixture.cached,
            };
            std::fs::remove_file(removed).unwrap();
        });
        ProcessWorkspace::from_scan(&fixture.policy, scan).unwrap();
    }
}

#[test]
fn hard_links_moved_to_a_scanned_directory_are_recounted() {
    let fixture = Fixture::new();
    std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
    let scan = fixture.scan_during(|| {
        std::fs::rename(
            &fixture.cached,
            fixture.object.with_file_name("moved.rmeta"),
        )
        .unwrap();
    });
    ProcessWorkspace::from_scan(&fixture.policy, scan).unwrap();
}

#[test]
fn hard_links_deleted_during_a_scan_are_discarded() {
    let fixture = Fixture::new();
    std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
    let scan = fixture.scan_during(|| {
        std::fs::remove_file(&fixture.object).unwrap();
        std::fs::remove_file(&fixture.cached).unwrap();
    });
    ProcessWorkspace::from_scan(&fixture.policy, scan).unwrap();
}

#[test]
fn a_repeated_scan_rejects_external_hard_links() {
    let fixture = Fixture::new();
    let outside = fixture.directory.join("outside.rmeta");
    let scan = fixture.scan_during(|| {
        std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
        std::fs::hard_link(&fixture.object, &outside).unwrap();
    });
    let error = ProcessWorkspace::from_scan(&fixture.policy, scan)
        .err()
        .unwrap();
    assert!(error.to_string().contains("2 of 3 links found"));
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "metadata");
}

#[test]
fn a_repeated_scan_rejects_hard_links_across_access_boundaries() {
    let fixture = Fixture::new();
    let protected = fixture.policy.root().join(".git/metadata.rmeta");
    std::fs::create_dir(protected.parent().unwrap()).unwrap();
    let scan = fixture.scan_during(|| {
        std::fs::hard_link(&fixture.object, &fixture.cached).unwrap();
        std::fs::hard_link(&fixture.object, &protected).unwrap();
    });
    assert!(ProcessWorkspace::from_scan(&fixture.policy, scan).is_err());
    assert_eq!(std::fs::read_to_string(protected).unwrap(), "metadata");
}
