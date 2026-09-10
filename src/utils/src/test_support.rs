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
            false
                if stderr.contains("sandbox-exec: sandbox_apply: Operation not permitted")
                    || stderr.contains(
                        "bwrap: Creating new namespace failed: Operation not permitted",
                    )
                    || stderr.contains("bwrap: setting up uid map: Permission denied")
                    || stderr.contains("bwrap: No permissions to create a new namespace")
                    || stderr.contains("bwrap: No permissions to create new namespace") =>
            {
                Ok(Self::Restricted(stderr))
            }
            false => Err(anyhow::anyhow!(
                "Sandbox probe failed with {}: {stderr}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout)
            )),
        }
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
            let output = result?;
            match output.status {
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
            }
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
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(status: i32, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(status << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn successful_probe_enables_sandbox_tests() {
        assert!(matches!(
            SandboxAvailability::from_output(output(0, "")).unwrap(),
            SandboxAvailability::Available
        ));
    }

    #[test]
    fn restricted_runners_skip_sandbox_tests() {
        for message in [
            "sandbox-exec: sandbox_apply: Operation not permitted",
            "bwrap: Creating new namespace failed: Operation not permitted",
            "bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces.",
            "bwrap: setting up uid map: Permission denied",
            "bwrap: No permissions to create a new namespace",
            "bwrap: No permissions to create new namespace, likely because the kernel does not allow non-privileged user namespaces.",
        ] {
            assert!(matches!(
                SandboxAvailability::from_output(output(1, message)).unwrap(),
                SandboxAvailability::Restricted(reason) if reason == message
            ));
        }
    }

    #[test]
    fn unexpected_probe_failures_are_not_skipped() {
        for message in [
            "",
            "sandbox-exec: invalid profile",
            "bwrap: Unknown option",
            "Cannot load libkrun: Permission denied",
        ] {
            assert!(SandboxAvailability::from_output(output(1, message)).is_err());
        }
    }

    #[test]
    fn fixture_permission_denials_are_skipped() {
        assert!(
            permitted::<()>(
                "create a fixture",
                Err(std::io::ErrorKind::PermissionDenied.into())
            )
            .is_none()
        );
    }

    #[test]
    #[should_panic(expected = "Could not create a fixture")]
    fn unexpected_fixture_errors_are_not_skipped() {
        permitted::<()>("create a fixture", Err(std::io::ErrorKind::NotFound.into()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permissive_parent_sandboxes_skip_nested_sandbox_tests() {
        if sandbox_available() {
            let result = std::process::Command::new("/usr/bin/sandbox-exec")
                .env_remove("JOE_SANDBOX_REQUIRED")
                .args(["-p", "(version 1)(allow default)"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sandbox::tests::drains_both_pipes_beyond_pipe_capacity",
                    "--nocapture",
                ])
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(result.status.success(), "{stderr}");
            assert!(stderr.contains("Skipping test:"), "{stderr}");
        }
    }
}
