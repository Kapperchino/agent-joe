use std::process::Output;
use tokio::process::Command;

pub async fn output(command: Command) -> anyhow::Result<Output> {
    #[cfg(unix)]
    {
        unix::output(command).await
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        Err(anyhow::anyhow!(
            "Managed process cleanup is unsupported on this platform"
        ))
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::execution::{ExecutionScope, ResourceKind};
    use std::process::Stdio;
    use tokio::{
        io::AsyncReadExt,
        process::{Child, ChildStderr, ChildStdout},
    };
    use tokio_util::sync::CancellationToken;

    struct ProcessGroup(u32);
    impl ProcessGroup {
        fn kill(&self) {
            unsafe {
                libc::kill(-(self.0 as i32), libc::SIGKILL);
            }
        }
    }

    struct RunningProcess {
        child: Child,
        group: ProcessGroup,
        stdout: ChildStdout,
        stderr: ChildStderr,
    }
    impl RunningProcess {
        async fn spawn(mut command: Command) -> anyhow::Result<Self> {
            command
                .process_group(0)
                .kill_on_drop(true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            match command.spawn() {
                Ok(mut child) => match (child.id(), child.stdout.take(), child.stderr.take()) {
                    (Some(pid), Some(stdout), Some(stderr)) => Ok(Self {
                        child,
                        group: ProcessGroup(pid),
                        stdout,
                        stderr,
                    }),
                    _ => {
                        let _ = child.start_kill();
                        let _ = child.wait().await;
                        Err(anyhow::anyhow!(
                            "Spawned process is missing its ID or pipes"
                        ))
                    }
                },
                Err(error) => Err(error.into()),
            }
        }

        async fn run(self, cancel: CancellationToken) -> anyhow::Result<Output> {
            let Self {
                mut child,
                group,
                mut stdout,
                mut stderr,
            } = self;
            let completion = async {
                let read = async {
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    tokio::try_join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err))
                        .map(|_| (out, err))
                };
                let wait = async {
                    let status = child.wait().await;
                    group.kill();
                    status
                };
                tokio::try_join!(wait, read)
                    .map(|(status, (stdout, stderr))| Output {
                        status,
                        stdout,
                        stderr,
                    })
                    .map_err(anyhow::Error::from)
            };
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(anyhow::anyhow!("Process cancelled")),
                result = completion => result,
            };
            if result.is_err() {
                group.kill();
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            result
        }
    }

    pub async fn output(command: Command) -> anyhow::Result<Output> {
        let scope = ExecutionScope::current();
        if scope.cancel.is_cancelled() {
            Err(anyhow::anyhow!("Process cancelled before launch"))
        } else {
            let cancel = scope.cancel.child_token();
            let _guard = cancel.clone().drop_guard();
            match RunningProcess::spawn(command).await {
                Ok(process) => {
                    let registration = scope.register(
                        ResourceKind::Process,
                        format!("Process {}", process.group.0),
                    );
                    scope
                        .tasks
                        .spawn(async move {
                            let _registration = registration;
                            process.run(cancel).await
                        })
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(std::convert::identity)
                }
                Err(error) => Err(error),
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::execution::ExecutionScope;
    use std::{io::Write, path::PathBuf, time::Duration};

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
                        .args(["--exact", "process::tests::process_fixture", "--nocapture"])
                        .env("JOE_M2_PROCESS_FIXTURE", "descendant")
                        .spawn()
                        .unwrap();
                    for _ in 0..500 {
                        if marker.with_extension("child").exists() {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    std::fs::write(&marker, format!("{} {}", std::process::id(), child.id()))
                        .unwrap();
                }
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            _ => {}
        }
    }

    fn fixture(mode: &str, marker: &PathBuf) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process::tests::process_fixture", "--nocapture"])
            .env("JOE_M2_PROCESS_FIXTURE", mode)
            .env("JOE_M2_PROCESS_MARKER", marker);
        command
    }

    #[tokio::test]
    async fn drains_both_pipes_beyond_pipe_capacity() {
        let command = fixture("pipes", &PathBuf::new());
        let result = tokio::time::timeout(Duration::from_secs(5), output(command))
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
            let marker =
                std::env::temp_dir().join(format!("joe-m2-process-{}", uuid::Uuid::new_v4()));
            let command = fixture("tree", &marker);
            let scope = ExecutionScope::default();
            let task_scope = scope.clone();
            let task = tokio::spawn(async move { task_scope.enter(output(command)).await });
            let pids = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(text) = tokio::fs::read_to_string(&marker).await {
                        if text.split_whitespace().count() == 2 {
                            break text;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
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
            std::fs::remove_file(&marker).unwrap();
            std::fs::remove_file(marker.with_extension("child")).unwrap();
        }
    }
}
