#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Output;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::sync::OnceLock;

pub fn permitted<T>(operation: &str, result: std::io::Result<T>) -> Option<T> {
    match result {
        Ok(resource) => Some(resource),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("Skipping test fixture: cannot {operation} in this runner: {error}");
            None
        }
        Err(error) => panic!("Could not {operation}: {error}"),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[derive(Debug)]
enum SandboxAvailability {
    Available,
    Restricted(String),
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl SandboxAvailability {
    fn probe() -> anyhow::Result<Self> {
        #[cfg(target_os = "linux")]
        let availability = Self::kvm(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/kvm"),
        )?;
        #[cfg(target_os = "macos")]
        let availability = Self::Available;
        match availability {
            Self::Available => Self::probe_sandbox(),
            restricted => Ok(restricted),
        }
    }

    #[cfg(any(test, target_os = "linux"))]
    fn kvm<T>(result: std::io::Result<T>) -> anyhow::Result<Self> {
        match result {
            Ok(_) => Ok(Self::Available),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(Self::Restricted(format!(
                    "Cannot access /dev/kvm to start a sandbox VM: {error}"
                )))
            }
            Err(error) => Err(error.into()),
        }
    }

    fn probe_sandbox() -> anyhow::Result<Self> {
        std::thread::spawn(|| {
            let directory =
                std::env::temp_dir().join(format!("joe-krun-probe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory)?;
            let result = probe_guest(&directory);
            std::fs::remove_dir_all(&directory)?;
            result
        })
        .join()
        .map_err(|_| anyhow::anyhow!("libkrun probe thread panicked"))?
    }

    fn from_output(output: Output) -> anyhow::Result<Self> {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        match output.status.success() {
            true => Ok(Self::Available),
            false => Self::restriction(&stderr).ok_or_else(|| {
                anyhow::anyhow!(
                    "Sandbox probe failed with {}: {stderr}\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout)
                )
            }),
        }
    }

    fn from_error(error: anyhow::Error) -> anyhow::Result<Self> {
        Self::restriction(&format!("{error:#}")).ok_or(error)
    }

    fn restriction(reason: &str) -> Option<Self> {
        [
            "sandbox-exec: sandbox_apply: Operation not permitted",
            "bwrap: Creating new namespace failed: Operation not permitted",
            "bwrap: setting up uid map: Permission denied",
            "bwrap: No permissions to create a new namespace",
            "bwrap: No permissions to create new namespace",
        ]
        .iter()
        .any(|message| reason.contains(message))
        .then(|| Self::Restricted(reason.to_owned()))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn probe_guest(directory: &std::path::Path) -> anyhow::Result<SandboxAvailability> {
    struct Probe;
    impl crate::sandbox::sealed::Operation for Probe {
        fn into_command(self) -> tokio::process::Command {
            tokio::process::Command::new("/bin/true")
        }
    }
    impl crate::sandbox::SandboxOperation for Probe {}
    let scope = crate::execution::ExecutionScope::with_workspace(
        crate::workspace::WorkspacePolicy::workspace(directory.into())?,
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let result = scope
                .enter(crate::sandbox::Sandbox::capture(Probe, 15))
                .await;
            scope.finish().await;
            result
                .and_then(|output| match output.status {
                    crate::process::ProcessStatus::Exited => {
                        use std::os::unix::process::ExitStatusExt;
                        SandboxAvailability::from_output(Output {
                            status: std::process::ExitStatus::from_raw(
                                output.exit_code.unwrap_or(125) << 8,
                            ),
                            stdout: output.stdout.into_bytes(),
                            stderr: output.stderr.into_bytes(),
                        })
                    }
                    status => Err(anyhow::anyhow!(
                        "libkrun guest probe failed: {status:?}: {:?}",
                        output.error
                    )),
                })
                .or_else(SandboxAvailability::from_error)
        })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn sandbox_available() -> bool {
    static AVAILABILITY: OnceLock<SandboxAvailability> = OnceLock::new();
    match AVAILABILITY.get_or_init(|| {
        SandboxAvailability::probe().expect("Could not check process sandbox support")
    }) {
        SandboxAvailability::Available => true,
        SandboxAvailability::Restricted(reason) => {
            eprintln!("Skipping test: the runner prevents creating a process sandbox: {reason}");
            assert!(
                std::env::var_os("JOE_SANDBOX_REQUIRED").is_none(),
                "Required libkrun sandbox unavailable: {reason}"
            );
            false
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
#[path = "test_support/tests.rs"]
mod tests;
