use super::*;

struct Fixture {
    directory: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let workspace = directory.join("project");
        std::fs::create_dir(&workspace).unwrap();
        Self {
            directory,
            workspace,
        }
    }

    fn cache(&self) -> BuildCache {
        BuildCache::new(self.directory.join("cache"), &self.workspace).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[tokio::test]
async fn artifacts_survive_cache_reopen_and_workspace_deletion() {
    let fixture = Fixture::new();
    let cache = fixture.cache();
    let lease = cache.lease(&[]).await.unwrap();
    std::fs::write(cache.path().join("artifact"), "compiled").unwrap();
    drop(lease);
    drop(cache);
    std::fs::remove_dir_all(&fixture.workspace).unwrap();
    let cache = fixture.cache();
    let _lease = cache.lease(&[]).await.unwrap();
    assert_eq!(
        std::fs::read(cache.path().join("artifact")).unwrap(),
        b"compiled"
    );
    assert!(!cache.path().join("lock").exists());
}

#[tokio::test]
async fn leases_serialize_independent_cache_handles_and_can_be_cancelled() {
    let fixture = Fixture::new();
    let first = fixture.cache();
    let second = fixture.cache();
    let lease = first.lease(&[]).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(75), second.lease(&[]))
            .await
            .is_err()
    );
    let cancel = CancellationToken::new();
    let waiting = second.lease(std::slice::from_ref(&cancel));
    let cancelling = async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(waiting, cancelling);
    assert!(result.is_err());
    drop(lease);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), second.lease(&[]))
            .await
            .unwrap()
            .is_ok()
    );
}

#[test]
fn cache_cannot_overlap_the_workspace() {
    let fixture = Fixture::new();
    assert!(BuildCache::new(fixture.workspace.join("cache"), &fixture.workspace).is_err());
    let cache = fixture.cache();
    assert!(BuildCache::new(fixture.directory.join("cache"), cache.path()).is_err());
}

#[tokio::test]
async fn lock_symlinks_and_hardlinks_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let cache = fixture.cache();
    let destination = fixture.directory.join("cache/lock");
    let source = fixture.directory.join("other");
    std::fs::write(&source, "private").unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::os::unix::fs::symlink(&source, &destination).unwrap();
    assert!(cache.lease(&[]).await.is_err());
    std::fs::remove_file(&destination).unwrap();
    match std::fs::hard_link(&source, &destination) {
        Ok(()) => assert!(cache.lease(&[]).await.is_err()),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("Skipping hardlink assertion: runner denies hardlinks: {error}");
        }
        Err(error) => panic!("Cannot create hardlink fixture: {error}"),
    }
}
