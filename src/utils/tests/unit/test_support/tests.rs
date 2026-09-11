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
