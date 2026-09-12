use super::*;

struct Fixture {
    root: PathBuf,
    outside: PathBuf,
    temporary: TemporaryDirectory,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("joe-temporary-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let outside = root.join("outside");
        let parent = root.join("workspace");
        std::fs::create_dir(&outside).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(outside.join("sentinel"), "untouched").unwrap();
        let temporary =
            TemporaryDirectory::create(open_directory(&parent).unwrap(), &parent).unwrap();
        Self {
            root,
            outside,
            temporary,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn replaced_session_paths_cannot_create_or_remove_outside_entries() {
    let fixture = Fixture::new();
    std::fs::rename(fixture.temporary.path(), fixture.root.join("original")).unwrap();
    std::os::unix::fs::symlink(&fixture.outside, fixture.temporary.path()).unwrap();
    assert!(fixture.temporary.child().is_err());
    fixture.temporary.remove();
    assert_eq!(
        std::fs::read_to_string(fixture.outside.join("sentinel")).unwrap(),
        "untouched"
    );
    assert_eq!(std::fs::read_dir(&fixture.outside).unwrap().count(), 1);
    assert!(fixture.temporary.path().is_symlink());
}

#[test]
fn replaced_ancestors_and_directory_identities_are_rejected() {
    let fixture = Fixture::new();
    let parent = fixture.temporary.path().parent().unwrap();
    std::fs::rename(parent, fixture.root.join("original")).unwrap();
    std::os::unix::fs::symlink(&fixture.outside, parent).unwrap();
    assert!(fixture.temporary.child().is_err());
    fixture.temporary.remove();
    assert_eq!(std::fs::read_dir(&fixture.outside).unwrap().count(), 1);
    std::fs::remove_file(parent).unwrap();
    std::fs::create_dir(parent).unwrap();
    std::fs::create_dir(fixture.temporary.path()).unwrap();
    std::fs::write(fixture.temporary.path().join("replacement"), "untouched").unwrap();
    assert!(fixture.temporary.child().is_err());
    fixture.temporary.remove();
    assert_eq!(
        std::fs::read_to_string(fixture.temporary.path().join("replacement")).unwrap(),
        "untouched"
    );
}

#[test]
fn cleanup_removes_owned_contents_without_following_child_links() {
    let fixture = Fixture::new();
    let child = fixture.temporary.child().unwrap();
    std::os::unix::fs::symlink(&fixture.outside, child.path().join("guest/outside")).unwrap();
    std::fs::write(child.path().join("guest/output"), "owned").unwrap();
    child.remove();
    assert!(!child.path().exists());
    assert_eq!(
        std::fs::read_to_string(fixture.outside.join("sentinel")).unwrap(),
        "untouched"
    );
    fixture.temporary.remove();
    assert!(!fixture.temporary.path().exists());
}
