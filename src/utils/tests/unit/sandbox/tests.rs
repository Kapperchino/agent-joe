use super::*;
use crate::execution::ExecutionScope;
use std::{path::PathBuf, time::Duration};

struct Fixture {
    directory: PathBuf,
    root: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-process-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(directory.join("project")).unwrap();
        std::fs::create_dir(directory.join("outside")).unwrap();
        let directory = directory.canonicalize().unwrap();
        Self {
            root: directory.join("project"),
            outside: directory.join("outside"),
            directory,
        }
    }

    fn scope(&self) -> ExecutionScope {
        ExecutionScope::with_workspace(
            crate::workspace::WorkspacePolicy::workspace(self.root.clone()).unwrap(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn process_fixture() {
    if std::env::var("JOE_SANDBOX_FIXTURE").as_deref() == Ok("environment-parent") {
        assert!(std::env::var_os("JOE_INHERITED_SECRET").is_some());
        let project = Fixture::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime
            .block_on(
                project
                    .scope()
                    .enter(output(fixture("environment-child", &PathBuf::new()))),
            )
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

async fn output(command: Command) -> anyhow::Result<Output> {
    execute(command, ProcessLimits::default()).await
}

fn boot_id() -> Command {
    let mut command = Command::new("/usr/bin/cat");
    command.arg("/proc/sys/kernel/random/boot_id");
    command
}

#[tokio::test]
async fn new_outside_hard_links_block_commands_without_restarting_the_vm() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        let first = scope.enter(output(boot_id())).await.unwrap();
        assert!(first.status.success());
        let outside = project.outside.join("secret");
        let alias = project.root.join("alias");
        std::fs::write(&outside, "untouched").unwrap();
        std::fs::hard_link(&outside, &alias).unwrap();
        let mut command = Command::new("/usr/bin/python3");
        command.args([
            "-c",
            "from pathlib import Path; Path('alias').write_text('changed')",
        ]);
        let error = scope.enter(output(command)).await.unwrap_err();
        assert!(format!("{error:#}").contains("Hard links must remain within workspace"));
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        std::fs::remove_file(alias).unwrap();
        let next = scope.enter(output(boot_id())).await.unwrap();
        assert!(next.status.success());
        assert_eq!(first.stdout, next.stdout);
        scope.finish().await;
    }
}

#[tokio::test]
async fn protected_paths_created_after_startup_remain_protected() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        let first = scope.enter(output(boot_id())).await.unwrap();
        assert!(first.status.success());
        let worktree = project.root.join(".joe-worktrees/checkout");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: /protected/metadata").unwrap();
        std::fs::create_dir(project.root.join(".turbo-code")).unwrap();
        std::fs::write(project.root.join(".turbo-code/secret"), "private").unwrap();
        let protected = scope
            .workspace()
            .unwrap()
            .process_protected_paths()
            .unwrap();
        assert!(protected.contains(&worktree.join(".git")));
        assert!(protected.contains(&project.root.join(".turbo-code")));
        let result = scope
            .enter(output(fixture("new-protected-paths", &PathBuf::new())))
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join(".git")).unwrap(),
            "gitdir: /protected/metadata"
        );
        assert_eq!(
            std::fs::read_to_string(project.root.join(".turbo-code/secret")).unwrap(),
            "private"
        );
        let next = scope.enter(output(boot_id())).await.unwrap();
        assert!(next.status.success());
        assert_eq!(first.stdout, next.stdout);
        scope.finish().await;
    }
}

struct DynamicProtection {
    workspace: workspace::SandboxWorkspace,
}

impl sandbox::workspace::Workspace for DynamicProtection {
    fn root(&self) -> &std::path::Path {
        self.workspace.root()
    }

    fn prepare(&self) -> anyhow::Result<sandbox::workspace::WorkspaceProtection> {
        let mut protection = self.workspace.prepare()?;
        protection.read_only.extend(
            ["metadata", "pointer"]
                .into_iter()
                .map(|path| self.root().join(path))
                .filter(|path| path.exists()),
        );
        protection.hidden.extend(
            ["saved-state", "secret-file"]
                .into_iter()
                .map(|path| self.root().join(path))
                .filter(|path| path.exists()),
        );
        Ok(protection)
    }

    fn read(&self, path: &std::path::Path) -> anyhow::Result<String> {
        self.workspace.read(path)
    }

    fn create_parent_dirs(&self, path: &std::path::Path) -> anyhow::Result<()> {
        self.workspace.create_parent_dirs(path)
    }

    fn link_process_cache(
        &self,
        source: &std::path::Path,
        destination: &std::path::Path,
    ) -> anyhow::Result<()> {
        self.workspace.link_process_cache(source, destination)
    }
}

#[tokio::test]
async fn command_mounts_refresh_read_only_and_hidden_files_and_directories() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        let sandbox = sandbox::Sandbox::new(
            std::sync::Arc::new(DynamicProtection {
                workspace: workspace::SandboxWorkspace::new(scope.workspace().unwrap()),
            }),
            scope.tasks.clone(),
            scope.cancel.clone(),
        );
        let first = sandbox
            .capture(boot_id(), ProcessLimits::default(), vec![])
            .await
            .unwrap();
        assert!(first.success());
        for path in ["metadata", "saved-state"] {
            std::fs::create_dir(project.root.join(path)).unwrap();
            std::fs::write(project.root.join(path).join("file"), "original").unwrap();
        }
        for path in ["pointer", "secret-file"] {
            std::fs::write(project.root.join(path), "original").unwrap();
        }
        let result = sandbox
            .capture(
                fixture("dynamic-mounts", &PathBuf::new()),
                ProcessLimits::default(),
                vec![],
            )
            .await
            .unwrap();
        assert!(result.success(), "{}", result.stderr);
        for path in [
            "metadata/file",
            "pointer",
            "saved-state/file",
            "secret-file",
        ] {
            assert_eq!(
                std::fs::read_to_string(project.root.join(path)).unwrap(),
                "original"
            );
        }
        let next = sandbox
            .capture(boot_id(), ProcessLimits::default(), vec![])
            .await
            .unwrap();
        assert!(next.success(), "{}", next.stderr);
        assert_eq!(first.stdout, next.stdout);
        scope.finish().await;
    }
}

fn fixture(mode: &str, marker: &std::path::Path) -> Command {
    let mut command = Command::new("/usr/bin/python3");
    command
        .args(["-c", include_str!("fixture.py")])
        .env("JOE_SANDBOX_FIXTURE", mode)
        .env(
            "JOE_SANDBOX_MARKER",
            PathBuf::from("/workspace").join(marker.file_name().unwrap_or_default()),
        );
    command
}

#[test]
fn inherited_credentials_and_sockets_are_removed() {
    if crate::test_support::sandbox_available() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sandbox::tests::process_fixture", "--nocapture"])
            .env("JOE_SANDBOX_FIXTURE", "environment-parent")
            .env("JOE_INHERITED_SECRET", "fixture-secret")
            .env("SSH_AUTH_SOCK", "/fixture/host-agent")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[tokio::test]
async fn sandbox_session_survives_commands_timeouts_and_turn_cleanup_until_shutdown() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        scope.sandbox().unwrap().start().await.unwrap();
        let boot = || {
            let mut command = Command::new("/usr/bin/cat");
            command.arg("/proc/sys/kernel/random/boot_id");
            command
        };
        let first_turn = scope.child();
        let first = first_turn.enter(output(boot())).await.unwrap();
        assert!(first.status.success());
        first_turn.finish().await;
        let second_turn = scope.child();
        let marker = project.root.join("timeout");
        let timed_out = second_turn
            .enter(execute(
                fixture("tree", &marker),
                ProcessLimits::new(Duration::from_millis(500), 1024 * 1024).unwrap(),
            ))
            .await
            .unwrap_err();
        assert!(timed_out.to_string().contains("time limit"));
        let heartbeat = std::fs::read(marker.with_extension("child")).unwrap();
        let second = second_turn.enter(output(boot())).await.unwrap();
        assert!(second.status.success());
        assert_eq!(first.stdout, second.stdout);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            std::fs::read(marker.with_extension("child")).unwrap(),
            heartbeat
        );
        second_turn.finish().await;
        let interrupted = scope.child();
        let task_scope = interrupted.clone();
        let marker = project.root.join("cancelled");
        let command = fixture("tree", &marker);
        let running = tokio::spawn(async move { task_scope.enter(output(command)).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !marker.with_extension("child").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        interrupted.finish().await;
        assert!(running.await.unwrap().is_err());
        let after_cancel = scope.enter(output(boot())).await.unwrap();
        assert!(after_cancel.status.success());
        assert_eq!(first.stdout, after_cancel.stdout);
        scope.finish().await;
        assert!(scope.tasks.is_empty());
        assert!(scope.sandbox().unwrap().start().await.is_err());
        assert!(
            std::fs::read_dir(project.root.join("target/.joe/tmp"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[tokio::test]
async fn cargo_observes_atomic_source_edits_in_a_running_sandbox() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(
            project.root.join("Cargo.toml"),
            "[package]\nname = 'live_edits'\nversion = '0.1.0'\nedition = '2024'\n",
        )
        .unwrap();
        let scope = project.scope();
        let source = std::path::Path::new("src/lib.rs");
        scope
            .workspace()
            .unwrap()
            .write(source, "#[test] fn current() { assert_eq!(1, 1); }")
            .unwrap();
        scope.sandbox().unwrap().start().await.unwrap();
        let first = scope
            .enter(crate::cargo::Cargo::cargo_test(None, None))
            .await
            .unwrap();
        assert!(matches!(first, crate::cargo::CargoTest::TestPasses { .. }));
        scope
            .workspace()
            .unwrap()
            .write(source, "#[test] fn current() { assert_eq!(1, 2); }")
            .unwrap();
        let changed = scope
            .enter(crate::cargo::Cargo::cargo_test(None, None))
            .await
            .unwrap();
        match changed {
            crate::cargo::CargoTest::TestFailed { output } => assert!(
                output.contains("assertion `left == right` failed"),
                "{output}"
            ),
            crate::cargo::CargoTest::TestPasses { .. } => panic!("Cargo reused stale source"),
        }
        scope.finish().await;
    }
}

#[tokio::test]
async fn sandbox_can_hold_build_sized_file_sets_open() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let result = project
            .scope()
            .enter(output(fixture("open-files", &PathBuf::new())))
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn sandbox_file_limits_do_not_depend_on_the_host_shell() {
    if crate::test_support::sandbox_available() {
        let result = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "ulimit -S -n 256 && exec \"$@\"",
                "joe-file-limit-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sandbox::tests::sandbox_can_hold_build_sized_file_sets_open",
                "--nocapture",
            ])
            .env("JOE_SANDBOX_REQUIRED", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[tokio::test]
async fn drains_both_pipes_beyond_pipe_capacity() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let command = fixture("pipes", &PathBuf::new());
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            project.scope().enter(output(command)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(result.status.success());
        assert!(result.stdout.len() >= 256 * 1024);
        assert_eq!(result.stderr.len(), 256 * 1024);
    }
}

#[tokio::test]
async fn temporary_workspaces_can_create_private_session_storage() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let saved = project.root.join(".turbo-code");
        std::fs::create_dir(&saved).unwrap();
        std::fs::write(saved.join("secret"), "saved session").unwrap();
        let scope = project.scope();
        let result = scope
            .enter(output(fixture("temporary-storage", &PathBuf::new())))
            .await
            .unwrap();
        scope.finish().await;
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(saved.join("secret")).unwrap(),
            "saved session"
        );
        assert!(
            std::fs::read_dir(project.root.join("target/.joe/tmp"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[tokio::test]
async fn sandbox_crate_observes_caller_cancellation_after_launch() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let marker = project.root.join("process");
        let scope = project.scope();
        let sandbox = sandbox::Sandbox::new(
            std::sync::Arc::new(workspace::SandboxWorkspace::new(scope.workspace().unwrap())),
            scope.tasks.clone(),
            scope.cancel.clone(),
        );
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancellations = vec![scope.cancel.clone(), cancel.clone()];
        let command = fixture("tree", &marker);
        let task = tokio::spawn(async move {
            sandbox
                .capture(command, ProcessLimits::default(), cancellations)
                .await
        });
        tokio::time::timeout(Duration::from_secs(15), async {
            while !marker.with_extension("child").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result.status, crate::process::ProcessStatus::Cancelled);
        tokio::time::timeout(Duration::from_secs(3), scope.finish())
            .await
            .unwrap();
        assert!(scope.tasks.is_empty());
        assert!(
            std::fs::read_dir(project.root.join("target/.joe/tmp"))
                .unwrap()
                .next()
                .is_none()
        );
        let heartbeat = std::fs::read(marker.with_extension("child")).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            std::fs::read(marker.with_extension("child")).unwrap(),
            heartbeat
        );
    }
}

#[tokio::test]
async fn cancellation_and_dropping_future_kill_descendants_and_reap_leader() {
    if crate::test_support::sandbox_available() {
        enum StopMode {
            CancelScope,
            DropFuture,
        }
        for mode in [StopMode::CancelScope, StopMode::DropFuture] {
            let project = Fixture::new();
            let marker = project.root.join("process");
            let command = fixture("tree", &marker);
            let scope = project.scope();
            let task_scope = scope.clone();
            let task = tokio::spawn(async move { task_scope.enter(output(command)).await });
            tokio::time::timeout(Duration::from_secs(15), async {
                while !marker.with_extension("child").exists() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            match mode {
                StopMode::DropFuture => task.abort(),
                StopMode::CancelScope => scope.cancel.cancel(),
            }
            let result = tokio::time::timeout(Duration::from_secs(3), task)
                .await
                .unwrap();
            assert!(result.is_err() || result.unwrap().is_err());
            tokio::time::timeout(Duration::from_secs(3), scope.finish())
                .await
                .unwrap();
            assert_eq!(scope.tasks.len(), 0);
            assert!(
                std::fs::read_dir(project.root.join("target/.joe/tmp"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            let heartbeat = std::fs::read(marker.with_extension("child")).unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                std::fs::read(marker.with_extension("child")).unwrap(),
                heartbeat
            );
            std::fs::remove_file(&marker).unwrap();
            std::fs::remove_file(marker.with_extension("child")).unwrap();
        }
    }
}
#[tokio::test]
async fn process_scope_denies_host_effects_and_inherits_into_new_sessions() {
    if crate::test_support::sandbox_available() {
        use crate::workspace::{RootAccess, RootSpec, WorkspacePolicy};
        let project = Fixture::new();
        std::fs::write(project.outside.join("secret"), "secret").unwrap();
        std::fs::write(project.root.join("input"), "input").unwrap();
        for directory in [".git", ".agents", ".codex", ".turbo-code", "readonly"] {
            std::fs::create_dir(project.root.join(directory)).unwrap();
        }
        std::fs::write(project.root.join(".turbo-code/config"), "credential").unwrap();
        std::fs::write(project.root.join("readonly/file"), "original").unwrap();
        std::os::unix::fs::symlink(&project.outside, project.root.join("escape")).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let policy = WorkspacePolicy::new(
            project.root.clone(),
            vec![
                RootSpec {
                    path: project.root.clone(),
                    access: RootAccess::ReadWrite,
                },
                RootSpec {
                    path: project.root.join("readonly"),
                    access: RootAccess::ReadOnly,
                },
            ],
        )
        .unwrap();
        let mut command = fixture("boundary", &PathBuf::new());
        command
            .env("JOE_OUTSIDE", &project.outside)
            .env("JOE_ENDPOINT", listener.local_addr().unwrap().to_string())
            .env("JOE_PARENT_PID", std::process::id().to_string());
        let result = ExecutionScope::with_workspace(policy)
            .enter(output(command))
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(project.outside.join("secret")).unwrap(),
            "secret"
        );
        assert_eq!(
            std::fs::read_to_string(project.root.join("allowed")).unwrap(),
            "allowed"
        );
    }
}

#[tokio::test]
async fn outside_hardlinks_and_missing_scope_prevent_execution() {
    let project = Fixture::new();
    std::fs::write(project.outside.join("secret"), "secret").unwrap();
    if crate::test_support::permitted(
        "create hard links",
        std::fs::hard_link(project.outside.join("secret"), project.root.join("linked")),
    )
    .is_some()
    {
        assert!(
            project
                .scope()
                .enter(output(fixture("pipes", &PathBuf::new())))
                .await
                .is_err()
        );
    }
    assert!(output(fixture("pipes", &PathBuf::new())).await.is_err());
    assert_eq!(
        std::fs::read_to_string(project.outside.join("secret")).unwrap(),
        "secret"
    );
}

#[tokio::test]
async fn excessive_output_terminates_the_process() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        let result = scope
            .enter(output(fixture("overflow", &PathBuf::new())))
            .await;
        assert!(result.unwrap_err().to_string().contains("stream limit"));
        scope.finish().await;
        assert!(scope.resources().is_empty());
    }
}

#[tokio::test]
async fn timeouts_and_cancelled_scopes_stop_execution() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let scope = project.scope();
        let marker = project.root.join("timeout");
        let limits = ProcessLimits::new(Duration::from_millis(500), 1024 * 1024).unwrap();
        let result = scope.enter(execute(fixture("tree", &marker), limits)).await;
        assert!(result.unwrap_err().to_string().contains("time limit"));
        scope.finish().await;
        assert!(scope.resources().is_empty());
        let cancelled = project.scope();
        cancelled.cancel.cancel();
        let result = cancelled
            .enter(output(fixture("pipes", &PathBuf::new())))
            .await;
        assert!(result.unwrap_err().to_string().contains("before launch"));
    }
}

#[tokio::test]
async fn unavailable_executables_do_not_disable_file_tools() {
    let project = Fixture::new();
    let scope = project.scope();
    let result = scope
        .enter(output(Command::new(project.outside.join("missing"))))
        .await;
    assert!(result.is_err() || !result.unwrap().status.success());
    scope
        .workspace()
        .unwrap()
        .write(std::path::Path::new("allowed"), "available")
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.root.join("allowed")).unwrap(),
        "available"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn protected_symlinks_cannot_add_host_mounts() {
    for name in [".git", ".turbo-code"] {
        let project = Fixture::new();
        std::os::unix::fs::symlink(&project.outside, project.root.join(name)).unwrap();
        assert!(
            project
                .scope()
                .enter(output(fixture("pipes", &PathBuf::new())))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn sandbox_build_metadata_compiles_without_downloading_a_runtime() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(
            project.root.join("Cargo.toml"),
            "[package]\nname = 'sandbox_build_fixture'\nversion = '0.1.0'\nedition = '2024'\n",
        )
        .unwrap();
        std::fs::write(
            project.root.join("build.rs"),
            include_str!("../../../../sandbox/build.rs"),
        )
        .unwrap();
        std::fs::write(
            project.root.join("src/lib.rs"),
            "pub const TARGET: &str = env!(\"JOE_SANDBOX_TARGET\");",
        )
        .unwrap();
        let result = project
            .scope()
            .enter(crate::cargo::Cargo::cargo_check())
            .await
            .unwrap();
        assert!(matches!(
            result,
            crate::cargo::CargoCheck::CheckPasses { .. }
        ));
        assert!(!project.root.join(".cache/agent-joe").exists());
    }
}

#[tokio::test]
async fn cargo_artifacts_are_reused_across_agents_and_restarts() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(
            project.root.join("Cargo.toml"),
            "[package]\nname = 'cache_fixture'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nitertools = '=0.15.0'\n",
        )
        .unwrap();
        std::fs::write(
            project.root.join("src/lib.rs"),
            "#[test] fn cached() { use itertools::Itertools; assert_eq!([1, 2].iter().join(\",\"), \"1,2\"); }",
        )
        .unwrap();
        std::fs::write(project.root.join("build.rs"), "fn main() {}").unwrap();
        let command = || {
            let mut command = Command::new("cargo");
            command.args(["test", "--offline", "--message-format=json"]);
            command
        };
        let scope = project.scope();
        let first = cargo_artifacts(scope.enter(output(command())).await.unwrap());
        assert!(first.iter().any(|artifact| !artifact.fresh));
        let sibling = scope.child();
        let peer = scope.child();
        let (second, third) = tokio::join!(
            sibling.enter(output(command())),
            peer.enter(output(command())),
        );
        for result in [second, third] {
            let artifacts = cargo_artifacts(result.unwrap());
            assert!(
                artifacts.iter().all(|artifact| artifact.fresh),
                "{artifacts:?}"
            );
        }
        sibling.finish().await;
        peer.finish().await;
        scope.finish().await;
        let restarted = cargo_artifacts(project.scope().enter(output(command())).await.unwrap());
        assert!(
            restarted.iter().all(|artifact| artifact.fresh),
            "{restarted:?}"
        );
        std::fs::write(
            project.root.join("src/lib.rs"),
            "#[test] fn changed() { panic!(\"changed source must execute\"); }",
        )
        .unwrap();
        let changed = project
            .scope()
            .enter(crate::cargo::Cargo::cargo_test(None, None))
            .await
            .unwrap();
        assert!(matches!(
            changed,
            crate::cargo::CargoTest::TestFailed { .. }
        ));
    }
}

fn cargo_artifacts(result: Output) -> Vec<cargo_metadata::Artifact> {
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let artifacts = cargo_metadata::Message::parse_stream(result.stdout.as_slice())
        .map(Result::unwrap)
        .filter_map(|message| match message {
            cargo_metadata::Message::CompilerArtifact(artifact) => Some(artifact),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!artifacts.is_empty());
    artifacts
}

#[tokio::test]
async fn cargo_build_scripts_proc_macros_and_tests_cannot_escape() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        let object = project.root.join("target/debug/deps/object.rcgu.o");
        let cached = project
            .root
            .join("target/debug/incremental/session/object.rcgu.o");
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&object, "cached object").unwrap();
        std::fs::hard_link(&object, &cached).unwrap();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(project.outside.join("secret"), "secret").unwrap();
        std::fs::write(project.root.join("Cargo.toml"), "[package]\nname = 'isolation_fixture'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nisolation_macro = { path = 'macros' }\nitertools = '=0.15.0'\n").unwrap();
        let outside = project.outside.join("secret");
        let check = format!("assert!(std::fs::write({outside:?}, \"changed\").is_err());");
        std::fs::write(
            project.root.join("build.rs"),
            format!("fn main() {{ {check} }}"),
        )
        .unwrap();
        std::fs::create_dir_all(project.root.join("macros/src")).unwrap();
        std::fs::write(project.root.join("macros/Cargo.toml"), "[package]\nname = 'isolation_macro'\nversion = '0.1.0'\nedition = '2024'\n[lib]\nproc-macro = true\n").unwrap();
        std::fs::write(project.root.join("macros/src/lib.rs"), format!(
            "#[proc_macro_attribute] pub fn confined(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {{ {check} item }}"
        )).unwrap();
        std::fs::write(
            project.root.join("src/lib.rs"),
            format!("#[isolation_macro::confined] #[test] fn confined() {{ use itertools::Itertools; assert_eq!([1, 2].iter().join(\",\"), \"1,2\"); {check} }}"),
        )
        .unwrap();
        let checked = project
            .scope()
            .enter(crate::cargo::Cargo::cargo_check())
            .await
            .unwrap();
        assert!(matches!(
            checked,
            crate::cargo::CargoCheck::CheckPasses { .. }
        ));
        let result = project
            .scope()
            .enter(crate::cargo::Cargo::cargo_test(None, None))
            .await
            .unwrap();
        match result {
            crate::cargo::CargoTest::TestPasses { .. } => {}
            crate::cargo::CargoTest::TestFailed { output } => panic!("{output}"),
        }
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "secret");
    }
}

#[tokio::test]
async fn cargo_downloads_dependencies_before_running_isolated_builds() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(
            project.root.join("Cargo.toml"),
            "[package]\nname = 'cargo_download_fixture'\nversion = '0.1.0'\nedition = '2024'\n[build-dependencies]\nitoa = '=1.0.14'\n[dev-dependencies]\neither = '=1.13.0'\n",
        )
        .unwrap();
        let network_check = "assert!(std::net::TcpStream::connect_timeout(&\"1.1.1.1:443\".parse().unwrap(), std::time::Duration::from_secs(1)).is_err());";
        std::fs::write(
            project.root.join("build.rs"),
            format!(
                "fn main() {{ assert_eq!(itoa::Buffer::new().format(42), \"42\"); {network_check} }}"
            ),
        )
        .unwrap();
        std::fs::write(
            project.root.join("src/lib.rs"),
            format!(
                "#[test] fn dependencies_are_available() {{ assert_eq!(either::Either::<u8, u8>::Left(42).left(), Some(42)); {network_check} }}"
            ),
        )
        .unwrap();
        let scope = project.scope();
        assert!(!project.root.join("Cargo.lock").exists());
        let result = scope
            .enter(crate::cargo::Cargo::cargo_test(None, None))
            .await
            .unwrap();
        match result {
            crate::cargo::CargoTest::TestPasses { .. } => {}
            crate::cargo::CargoTest::TestFailed { output } => panic!("{output}"),
        }
        let lock = std::fs::read_to_string(project.root.join("Cargo.lock")).unwrap();
        assert!(lock.contains("name = \"itoa\"\nversion = \"1.0.14\""));
        assert!(lock.contains("name = \"either\"\nversion = \"1.13.0\""));
        let result = scope
            .enter(crate::cargo::Cargo::cargo_check())
            .await
            .unwrap();
        assert!(matches!(
            result,
            crate::cargo::CargoCheck::CheckPasses { .. }
        ));
        assert_eq!(
            std::fs::read_to_string(project.root.join("Cargo.lock")).unwrap(),
            lock
        );
        scope.finish().await;
    }
}

#[tokio::test]
async fn changing_dependencies_prepares_new_packages_automatically() {
    if crate::test_support::sandbox_available() {
        let project = Fixture::new();
        std::fs::create_dir(project.root.join("src")).unwrap();
        std::fs::write(
            project.root.join("src/lib.rs"),
            "pub fn value() -> u8 { 1 }",
        )
        .unwrap();
        let manifest = "[package]\nname = 'dependency_fixture'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\neither = '=1.15.0'\n";
        std::fs::write(project.root.join("Cargo.toml"), manifest).unwrap();
        let scope = project.scope();
        let first = scope
            .enter(crate::cargo::Cargo::cargo_check())
            .await
            .unwrap();
        assert!(matches!(
            first,
            crate::cargo::CargoCheck::CheckPasses { .. }
        ));
        std::fs::write(
            project.root.join("Cargo.toml"),
            format!("{manifest}itoa = '=1.0.15'\n"),
        )
        .unwrap();
        let second = scope
            .enter(crate::cargo::Cargo::cargo_check())
            .await
            .unwrap();
        assert!(matches!(
            second,
            crate::cargo::CargoCheck::CheckPasses { .. }
        ));
        let lock = std::fs::read_to_string(project.root.join("Cargo.lock")).unwrap();
        assert!(lock.contains("name = \"itoa\"\nversion = \"1.0.15\""));
        assert!(lock.contains("name = \"either\"\nversion = \"1.15.0\""));
        scope.finish().await;
    }
}

#[tokio::test]
async fn redirected_cache_paths_cannot_escape_the_workspace() {
    let fixture = Fixture::new();
    let cache = fixture.root.join("target/.joe/linux");
    std::fs::create_dir_all(&cache).unwrap();
    std::os::unix::fs::symlink(&fixture.outside, cache.join("cargo")).unwrap();
    let result = fixture.scope().enter(output(Command::new("cargo"))).await;
    assert!(result.is_err());
    assert!(!fixture.outside.join("registry").exists());
}
