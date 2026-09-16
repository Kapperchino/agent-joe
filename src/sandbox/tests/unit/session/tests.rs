use super::*;
use std::path::{Path, PathBuf};
use tokio::process::Command;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("joe-startup-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self {
            root: root.canonicalize().unwrap(),
        }
    }

    fn command(&self) -> IsolatedCommand {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 'started\\n' >> \"$1\"; while [ ! -e \"$2\" ]; do sleep 0.01; done; printf 'joe-session:{\"event\":\"ready\"}\\n'; cat > /dev/null", "fixture"])
            .arg(self.root.join("started"))
            .arg(self.root.join("ready"));
        crate::isolation::fixture::command(command, self).unwrap()
    }
}

impl Workspace for Fixture {
    fn root(&self) -> &Path {
        &self.root
    }

    fn prepare(&self) -> anyhow::Result<crate::workspace::WorkspaceProtection> {
        Ok(crate::workspace::WorkspaceProtection::default())
    }

    fn read(&self, path: &Path) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(self.root.join(path))?)
    }

    fn create_parent_dirs(&self, path: &Path) -> anyhow::Result<()> {
        Ok(std::fs::create_dir_all(
            self.root
                .join(path)
                .parent()
                .context("Missing fixture parent")?,
        )?)
    }

    fn link_process_cache(&self, source: &Path, destination: &Path) -> anyhow::Result<()> {
        Ok(std::os::unix::fs::symlink(
            source,
            self.root.join(destination),
        )?)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn aborted_startup_waits_reuse_one_published_launcher() {
    let fixture = Arc::new(Fixture::new());
    let cancel = CancellationToken::new();
    let owner = Arc::new(SessionOwner::new(cancel.clone()));
    let tasks = TaskTracker::new();
    let start = || {
        let owner = owner.clone();
        let fixture = fixture.clone();
        let tasks = tasks.clone();
        tokio::spawn(async move {
            owner
                .initialize(async {
                    Session::start(fixture.command(), owner.cancel.child_token(), &tasks)
                })
                .await
        })
    };
    let first = start();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fixture.root.join("started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    first.abort();
    assert!(first.await.is_err_and(|error| error.is_cancelled()));
    assert!(owner.session.read().await.get().is_some());
    let second = start();
    let concurrent = start();
    std::fs::write(fixture.root.join("ready"), "ready").unwrap();
    let session = tokio::time::timeout(std::time::Duration::from_secs(5), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let concurrent = tokio::time::timeout(std::time::Duration::from_secs(5), concurrent)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&session, &concurrent));
    assert!(!session.cancel.is_cancelled());
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("started")).unwrap(),
        "started\n"
    );
    assert_eq!(tasks.len(), 1);
    cancel.cancel();
    tasks.close();
    tokio::time::timeout(std::time::Duration::from_secs(5), tasks.wait())
        .await
        .unwrap();
    assert!(session.ready().await.is_err());
    assert_eq!(
        std::fs::read_dir(fixture.root.join("target/.joe/tmp"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test]
async fn shutdown_waits_for_cleanup_and_allows_a_fresh_launcher() {
    let fixture = Fixture::new();
    let cancel = CancellationToken::new();
    let owner = SessionOwner::new(cancel.clone());
    let tasks = TaskTracker::new();
    owner.shutdown().await.unwrap();
    std::fs::write(fixture.root.join("ready"), "ready").unwrap();
    for _ in 0..2 {
        let session = owner
            .initialize(async {
                Session::start(fixture.command(), owner.cancel.child_token(), &tasks)
            })
            .await
            .unwrap();
        let temporary = fixture
            .root
            .join("target/.joe/tmp")
            .join(session.temporary.id().to_string());
        assert!(temporary.exists());
        tokio::time::timeout(std::time::Duration::from_secs(5), owner.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(!temporary.exists());
        assert!(session.ready().await.is_err());
        assert!(!cancel.is_cancelled());
        assert!(!owner.cancel.is_cancelled());
        assert!(owner.session.read().await.get().is_none());
    }
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("started")).unwrap(),
        "started\nstarted\n"
    );
    tasks.close();
    tokio::time::timeout(std::time::Duration::from_secs(5), tasks.wait())
        .await
        .unwrap();
}

#[tokio::test]
async fn launcher_errors_are_bounded_without_blocking_shutdown() {
    let fixture = Fixture::new();
    let tasks = TaskTracker::new();
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "head -c 131072 /dev/zero | tr '\\000' x >&2; exit 1"]);
    let prepared = crate::isolation::fixture::command(command, &fixture).unwrap();
    let session = Session::start(prepared, CancellationToken::new(), &tasks).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), session.ready())
        .await
        .unwrap();
    match result {
        Ok(_) => panic!("Failed launcher became ready"),
        Err(error) => {
            let message = error.to_string();
            assert!(message.ends_with(&"x".repeat(16384)), "{message}");
            assert!(message.len() < 17000);
        }
    }
    tasks.close();
    tokio::time::timeout(std::time::Duration::from_secs(5), tasks.wait())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(fixture.root.join("target/.joe/tmp"))
            .unwrap()
            .count(),
        0
    );
}
