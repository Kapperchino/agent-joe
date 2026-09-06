use super::*;
use crate::{execution::ExecutionScope, files::Files};

struct Fixture {
    directory: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-policy-{}", uuid::Uuid::new_v4()));
        let root = directory.join("workspace");
        let outside = directory.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        Self {
            directory,
            root,
            outside,
        }
    }

    fn policy(&self) -> WorkspacePolicy {
        WorkspacePolicy::workspace(self.root.clone()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn paths_are_relative_to_the_workspace_and_ordinary_file_operations_work() {
    let fixture = Fixture::new();
    let policy = fixture.policy();
    policy
        .write(Path::new("nested/Cargo.toml"), "fixture manifest")
        .unwrap();
    assert_eq!(
        policy.read(Path::new("nested/Cargo.toml")).unwrap(),
        "fixture manifest"
    );
    assert_eq!(
        policy
            .read(&fixture.root.join("nested/Cargo.toml"))
            .unwrap(),
        "fixture manifest"
    );
    policy
        .rename(Path::new("nested/Cargo.toml"), Path::new("moved/manifest"))
        .unwrap();
    assert_eq!(
        policy.entries(Path::new("moved")).unwrap()[0].name,
        "manifest"
    );
    policy.delete(Path::new("moved/manifest")).unwrap();
    assert!(policy.entries(Path::new("moved")).unwrap().is_empty());
    assert!(policy.read(Path::new("nested/Cargo.toml")).is_err());
}

#[test]
fn traversal_outside_roots_and_protected_paths_are_denied() {
    let fixture = Fixture::new();
    let policy = fixture.policy();
    std::fs::write(fixture.outside.join("secret"), "secret").unwrap();
    for path in [
        PathBuf::from("../outside/secret"),
        fixture.outside.join("secret"),
        fixture.root.join("../outside/secret"),
    ] {
        assert!(policy.read(&path).is_err());
        assert!(policy.write(&path, "changed").is_err());
        assert!(policy.delete(&path).is_err());
        assert!(policy.rename(&path, Path::new("stolen")).is_err());
    }
    for path in [
        ".git/config",
        "nested/.GIT/config",
        ".agents/rules",
        ".codex/config",
        ".turbo-code/config",
    ] {
        assert!(policy.write(Path::new(path), "changed").is_err());
    }
    std::fs::create_dir_all(fixture.root.join(".git")).unwrap();
    std::fs::write(fixture.root.join(".git/config"), "git configuration").unwrap();
    assert_eq!(
        policy.read(Path::new(".git/config")).unwrap(),
        "git configuration"
    );
    assert!(policy.read(Path::new(".turbo-code/config")).is_err());
    policy.write(Path::new("source"), "source").unwrap();
    assert!(
        policy
            .rename(Path::new("source"), Path::new(".git/config"))
            .is_err()
    );
    assert_eq!(policy.read(Path::new("source")).unwrap(), "source");
    assert_eq!(
        std::fs::read_to_string(fixture.outside.join("secret")).unwrap(),
        "secret"
    );
}

#[test]
fn more_specific_read_only_roots_override_parent_write_access() {
    let fixture = Fixture::new();
    let readonly = fixture.root.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    std::fs::write(readonly.join("file"), "original").unwrap();
    std::fs::write(fixture.outside.join("reference"), "reference").unwrap();
    let policy = WorkspacePolicy::new(
        fixture.root.clone(),
        vec![
            RootSpec {
                path: fixture.root.clone(),
                access: RootAccess::ReadWrite,
            },
            RootSpec {
                path: readonly.clone(),
                access: RootAccess::ReadOnly,
            },
        ],
    )
    .unwrap();
    assert_eq!(policy.read(Path::new("readonly/file")).unwrap(), "original");
    assert!(policy.read(&fixture.outside.join("reference")).is_err());
    assert!(policy.write(Path::new("readonly/file"), "changed").is_err());
    assert!(policy.delete(Path::new("readonly/file")).is_err());
    assert!(
        policy
            .write(&fixture.outside.join("new"), "changed")
            .is_err()
    );
    std::fs::rename(&readonly, fixture.root.join("renamed")).unwrap();
    assert!(policy.write(Path::new("renamed/file"), "changed").is_err());
    assert!(
        policy
            .create_parent_dirs(Path::new("renamed/nested/file"))
            .is_err()
    );
    assert!(!fixture.root.join("renamed/nested").exists());
}

#[test]
fn alternate_case_cannot_bypass_read_only_directory_handles() {
    let fixture = Fixture::new();
    let readonly = fixture.root.join("readonly");
    std::fs::create_dir(&readonly).unwrap();
    let policy = WorkspacePolicy::new(
        fixture.root.clone(),
        vec![
            RootSpec {
                path: fixture.root.clone(),
                access: RootAccess::ReadWrite,
            },
            RootSpec {
                path: readonly,
                access: RootAccess::ReadOnly,
            },
        ],
    )
    .unwrap();
    if fixture.root.join("READONLY").is_dir() {
        assert!(policy.write(Path::new("READONLY/file"), "changed").is_err());
        assert!(!fixture.root.join("readonly/file").exists());
    } else {
        policy
            .write(Path::new("READONLY/file"), "separate directory")
            .unwrap();
        assert!(!fixture.root.join("readonly/file").exists());
    }
}

#[cfg(unix)]
#[test]
fn symlink_swaps_hard_links_and_special_files_do_not_escape_the_policy() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let policy = fixture.policy();
    std::fs::write(fixture.outside.join("secret"), "secret").unwrap();
    symlink(fixture.outside.join("secret"), fixture.root.join("link")).unwrap();
    std::fs::hard_link(fixture.outside.join("secret"), fixture.root.join("hard")).unwrap();
    for path in ["link", "hard"] {
        assert!(policy.read(Path::new(path)).is_err());
        assert!(policy.write(Path::new(path), "changed").is_err());
        assert!(policy.rename(Path::new(path), Path::new("moved")).is_err());
    }
    std::fs::create_dir(fixture.root.join("slot")).unwrap();
    policy
        .check(Path::new("slot/secret"), Access::Write)
        .unwrap();
    std::fs::rename(fixture.root.join("slot"), fixture.root.join("retained")).unwrap();
    symlink(&fixture.outside, fixture.root.join("slot")).unwrap();
    assert!(policy.read(Path::new("slot/secret")).is_err());
    assert!(policy.write(Path::new("slot/secret"), "changed").is_err());
    assert!(
        policy
            .create_parent_dirs(Path::new("slot/new/file"))
            .is_err()
    );
    assert!(policy.delete(Path::new("slot/secret")).is_err());
    assert!(
        std::process::Command::new("mkfifo")
            .arg(fixture.root.join("pipe"))
            .status()
            .unwrap()
            .success()
    );
    assert!(policy.read(Path::new("pipe")).is_err());
    assert!(policy.write(Path::new("pipe"), "changed").is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.outside.join("secret")).unwrap(),
        "secret"
    );
    assert!(!fixture.outside.join("new").exists());
}

#[cfg(unix)]
#[test]
fn root_handles_survive_path_replacement_and_preserve_file_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let policy = fixture.policy();
    policy.write(Path::new("script"), "old").unwrap();
    std::fs::set_permissions(
        fixture.root.join("script"),
        std::fs::Permissions::from_mode(0o751),
    )
    .unwrap();
    let retained = fixture.directory.join("retained");
    std::fs::rename(&fixture.root, &retained).unwrap();
    symlink(&fixture.outside, &fixture.root).unwrap();
    policy.write(Path::new("script"), "new").unwrap();
    assert_eq!(
        std::fs::read_to_string(retained.join("script")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::metadata(retained.join("script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert!(!fixture.outside.join("script").exists());
}

#[tokio::test]
async fn missing_policies_fail_closed_and_children_inherit_the_same_policy() {
    let fixture = Fixture::new();
    assert!(
        Files::write_to_file(&fixture.root.join("file"), "denied")
            .await
            .is_err()
    );
    let scope = ExecutionScope::with_workspace(fixture.policy());
    let child = scope.child();
    assert!(std::sync::Arc::ptr_eq(
        &scope.workspace().unwrap(),
        &child.workspace().unwrap()
    ));
    child
        .enter(async {
            Files::write_to_file(Path::new("file"), "allowed")
                .await
                .unwrap();
            assert_eq!(
                Files::read_file(Path::new("file")).await.unwrap(),
                "allowed"
            );
            assert!(
                Files::write_to_file(&fixture.outside.join("file"), "denied")
                    .await
                    .is_err()
            );
        })
        .await;
    child.finish().await;
    assert!(
        child
            .enter(Files::write_to_file(Path::new("file"), "cancelled"))
            .await
            .is_err()
    );
    assert_eq!(
        scope.workspace().unwrap().read(Path::new("file")).unwrap(),
        "allowed"
    );
}

#[tokio::test]
async fn copying_preserves_binary_content_and_permissions_without_following_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let source = fixture.root.join("binary");
    let bytes = [0, 255, 254, 128];
    std::fs::write(&source, bytes).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o751)).unwrap();
    let scope = ExecutionScope::with_workspace(fixture.policy());
    scope
        .enter(async {
            Files::copy_file(Path::new("binary"), Path::new("copy"))
                .await
                .unwrap();
            assert_eq!(std::fs::read(fixture.root.join("copy")).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(fixture.root.join("copy"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o751
            );
            assert!(
                Files::copy_file(Path::new("binary"), &fixture.outside.join("copy"))
                    .await
                    .is_err()
            );
            std::fs::write(fixture.outside.join("secret"), b"secret").unwrap();
            symlink(fixture.outside.join("secret"), fixture.root.join("link")).unwrap();
            assert!(
                Files::copy_file(Path::new("binary"), Path::new("link"))
                    .await
                    .is_err()
            );
            assert_eq!(
                std::fs::read(fixture.outside.join("secret")).unwrap(),
                b"secret"
            );
        })
        .await;
}

#[tokio::test]
async fn searches_propagate_policy_denials_and_do_not_read_protected_storage() {
    let fixture = Fixture::new();
    std::fs::write(fixture.root.join("file"), "match").unwrap();
    std::fs::create_dir(fixture.root.join(".turbo-code")).unwrap();
    std::fs::write(fixture.root.join(".turbo-code/config"), "secret").unwrap();
    let scope = ExecutionScope::with_workspace(fixture.policy());
    scope
        .enter(async {
            assert!(
                crate::grep::Grep::grep(
                    "match",
                    vec![PathBuf::from("file"), PathBuf::from(".turbo-code/config")],
                    0,
                    0
                )
                .await
                .is_err()
            );
            assert!(
                crate::text_search::TextSearch::search_str(
                    "secret",
                    &PathBuf::from(".turbo-code/config")
                )
                .is_err()
            );
            assert!(
                crate::text_search::TextSearch::search_and_replace(
                    "secret",
                    "changed",
                    &PathBuf::from(".turbo-code/config")
                )
                .await
                .is_err()
            );
        })
        .await;
    assert_eq!(
        std::fs::read_to_string(fixture.root.join(".turbo-code/config")).unwrap(),
        "secret"
    );
}

#[test]
fn configuration_cannot_add_roots_outside_the_project() {
    let fixture = Fixture::new();
    for access in [RootAccess::ReadOnly, RootAccess::ReadWrite] {
        assert!(
            WorkspacePolicy::new(
                fixture.root.clone(),
                vec![RootSpec {
                    path: fixture.outside.clone(),
                    access,
                }]
            )
            .is_err()
        );
    }
}

#[test]
fn log_handles_do_not_follow_replacements_or_open_outside_aliases() {
    use std::io::Write;
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let policy = fixture.policy();
    let outside = fixture.outside.join("secret");
    std::fs::write(&outside, "secret").unwrap();
    let mut log = policy.open_append(Path::new("logs/stream")).unwrap();
    std::fs::rename(
        fixture.root.join("logs/stream"),
        fixture.root.join("logs/retained"),
    )
    .unwrap();
    symlink(&outside, fixture.root.join("logs/stream")).unwrap();
    log.write_all(b"entry").unwrap();
    assert!(policy.open_append(Path::new("logs/stream")).is_err());
    assert!(policy.open_append(&outside).is_err());
    std::fs::hard_link(&outside, fixture.root.join("linked")).unwrap();
    assert!(policy.open_append(Path::new("linked")).is_err());
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    assert_eq!(policy.read(Path::new("logs/retained")).unwrap(), "entry");
}
