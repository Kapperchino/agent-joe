use super::*;
use crate::execution::ExecutionScope;
use std::{io::Write, path::PathBuf, time::Duration};

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
    match std::env::var("JOE_M2_PROCESS_FIXTURE").as_deref() {
        Ok("pipes") => {
            std::io::stdout()
                .write_all(&vec![b'o'; 256 * 1024])
                .unwrap();
            std::io::stderr()
                .write_all(&vec![b'e'; 256 * 1024])
                .unwrap();
        }
        Ok(mode @ ("descendant" | "tree")) => {
            let marker = PathBuf::from(std::env::var_os("JOE_M2_PROCESS_MARKER").unwrap());
            if mode == "descendant" {
                std::fs::write(
                    marker.with_extension("child"),
                    std::process::id().to_string(),
                )
                .unwrap();
            } else {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "sandbox::tests::process_fixture", "--nocapture"])
                    .env("JOE_M2_PROCESS_FIXTURE", "descendant")
                    .spawn()
                    .unwrap();
                let mut attempts = 0;
                while !marker.with_extension("child").exists() && attempts < 500 {
                    std::thread::sleep(Duration::from_millis(5));
                    attempts += 1;
                }
                std::fs::write(&marker, format!("{} {}", std::process::id(), child.id())).unwrap();
            }
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        Ok("boundary") => {
            let root = std::env::current_dir().unwrap();
            let outside = PathBuf::from(std::env::var_os("JOE_OUTSIDE").unwrap());
            assert_eq!(std::env::var_os("HOME").unwrap(), root.as_os_str());
            assert!(std::env::var_os("JOE_INHERITED_SECRET").is_none());
            assert!(std::env::var_os("SSH_AUTH_SOCK").is_none());
            assert!(std::fs::read_to_string(outside.join("secret")).is_err());
            let volume_alias = PathBuf::from("/System/Volumes/Data")
                .join(outside.strip_prefix("/").unwrap())
                .join("secret");
            assert!(std::fs::read_to_string(volume_alias).is_err());
            assert!(std::fs::write(outside.join("secret"), "changed").is_err());
            assert!(std::fs::write(root.join("escape/secret"), "changed").is_err());
            assert!(std::fs::hard_link(outside.join("secret"), root.join("hardlink")).is_err());
            assert!(std::fs::hard_link(root.join("input"), root.join("new-link")).is_err());
            assert!(std::fs::rename(root.join("input"), outside.join("moved")).is_err());
            for path in [
                ".git/config",
                ".GIT/config",
                ".agents/rules",
                ".codex/config",
                "readonly/file",
                "READONLY/file",
            ] {
                assert!(
                    std::fs::write(root.join(path), "changed").is_err(),
                    "Write succeeded: {path}"
                );
            }
            assert!(
                std::fs::rename(root.join("readonly"), root.join("formerly-readonly")).is_err()
            );
            assert!(std::fs::read(root.join(".turbo-code/config")).is_err());
            let endpoint = std::env::var("JOE_ENDPOINT").unwrap();
            assert!(std::net::TcpStream::connect(endpoint).is_err());
            let parent = std::env::var("JOE_PARENT_PID")
                .unwrap()
                .parse::<i32>()
                .unwrap();
            assert_eq!(unsafe { libc::kill(parent, libc::SIGCONT) }, -1);
            std::fs::write(root.join("allowed"), "allowed").unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "sandbox::tests::process_fixture", "--nocapture"])
                .env("JOE_M2_PROCESS_FIXTURE", "escaped-session")
                .status()
                .unwrap();
            assert!(child.success());
        }
        Ok("escaped-session") => {
            assert!(unsafe { libc::setsid() } > 0);
            let outside = PathBuf::from(std::env::var_os("JOE_OUTSIDE").unwrap());
            assert!(std::fs::write(outside.join("secret"), "changed").is_err());
            assert!(std::net::TcpStream::connect(std::env::var("JOE_ENDPOINT").unwrap()).is_err());
        }
        Ok("overflow") => {
            let _ = std::io::stdout().write_all(&vec![b'x'; 17 * 1024 * 1024]);
        }

        Ok("environment-parent") => {
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
        Ok("environment-child") => {
            assert!(std::env::var_os("JOE_INHERITED_SECRET").is_none());
            assert!(std::env::var_os("SSH_AUTH_SOCK").is_none());
        }
        _ => {}
    }
}

async fn output(command: Command) -> anyhow::Result<Output> {
    execute(command, ProcessLimits::default()).await
}

fn fixture(mode: &str, marker: &PathBuf) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "sandbox::tests::process_fixture", "--nocapture"])
        .env("JOE_M2_PROCESS_FIXTURE", mode)
        .env("JOE_M2_PROCESS_MARKER", marker);
    command
}

#[test]
fn inherited_credentials_and_sockets_are_removed() {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "sandbox::tests::process_fixture", "--nocapture"])
        .env("JOE_M2_PROCESS_FIXTURE", "environment-parent")
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

#[tokio::test]
async fn drains_both_pipes_beyond_pipe_capacity() {
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

#[tokio::test]
async fn cancellation_and_dropping_future_kill_descendants_and_reap_leader() {
    for drop_future in [false, true] {
        let project = Fixture::new();
        let marker = project.root.join("process");
        let command = fixture("tree", &marker);
        let scope = project.scope();
        let task_scope = scope.clone();
        let task = tokio::spawn(async move { task_scope.enter(output(command)).await });
        let pids = tokio::time::timeout(Duration::from_secs(5), async {
            let mut pids = String::new();
            while pids.split_whitespace().count() != 2 {
                pids = tokio::fs::read_to_string(&marker).await.unwrap_or_default();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            pids
        })
        .await
        .unwrap();
        if drop_future {
            task.abort();
        } else {
            scope.cancel.cancel();
        }
        let result = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap();
        assert!(result.is_err() || result.unwrap().is_err());
        tokio::time::timeout(Duration::from_secs(3), scope.finish())
            .await
            .unwrap();
        assert_eq!(scope.tasks.len(), 0);
        #[cfg(target_os = "macos")]
        for pid in pids
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap())
        {
            tokio::time::timeout(Duration::from_secs(3), async {
                while unsafe { libc::kill(pid, 0) } == 0 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("child process survived cancellation");
        }
        #[cfg(target_os = "linux")]
        assert_eq!(tokio::fs::read_to_string(&marker).await.unwrap(), pids);
        std::fs::remove_file(&marker).unwrap();
        std::fs::remove_file(marker.with_extension("child")).unwrap();
    }
}
#[tokio::test]
async fn process_scope_denies_host_effects_and_inherits_into_new_sessions() {
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

#[tokio::test]
async fn preexisting_hardlinks_and_missing_scope_prevent_execution() {
    let project = Fixture::new();
    std::fs::write(project.outside.join("secret"), "secret").unwrap();
    std::fs::hard_link(project.outside.join("secret"), project.root.join("linked")).unwrap();
    assert!(
        project
            .scope()
            .enter(output(fixture("pipes", &PathBuf::new())))
            .await
            .is_err()
    );
    assert!(output(fixture("pipes", &PathBuf::new())).await.is_err());
    assert_eq!(
        std::fs::read_to_string(project.outside.join("secret")).unwrap(),
        "secret"
    );
}

#[tokio::test]
async fn excessive_output_terminates_the_process() {
    let project = Fixture::new();
    let scope = project.scope();
    let result = scope
        .enter(output(fixture("overflow", &PathBuf::new())))
        .await;
    assert!(result.unwrap_err().to_string().contains("stream limit"));
    scope.finish().await;
    assert!(scope.resources().is_empty());
}

#[tokio::test]
async fn timeouts_and_cancelled_scopes_stop_execution() {
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

#[tokio::test]
async fn unavailable_executables_do_not_disable_file_tools() {
    let project = Fixture::new();
    let scope = project.scope();
    let result = scope
        .enter(output(Command::new(project.outside.join("missing"))))
        .await;
    assert!(result.is_err());
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
async fn cargo_build_scripts_proc_macros_and_tests_cannot_escape() {
    let project = Fixture::new();
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
