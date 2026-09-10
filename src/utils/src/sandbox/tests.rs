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
        let result = project
            .scope()
            .enter(output(fixture("temporary-storage", &PathBuf::new())))
            .await
            .unwrap();
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
