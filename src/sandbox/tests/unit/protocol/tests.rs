use super::*;

#[cfg(target_os = "linux")]
#[test]
fn guest_workspace_mounts_and_cache_shutdown_are_isolated() {
    use std::io::Write;
    let mut child = std::process::Command::new("/usr/bin/python3")
        .args(["-I", "-c", include_str!("guest.py")])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("../../../guest.py"))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn workspace_selection_is_relative_to_the_shared_project() {
    let project = Path::new("/project");
    assert_eq!(
        CommandProtection::workspace_path(project, project).unwrap(),
        Path::new("/joe-project")
    );
    assert_eq!(
        CommandProtection::workspace_path(project, Path::new("/project/.joe-worktrees/session"))
            .unwrap(),
        Path::new("/joe-project/.joe-worktrees/session")
    );
    for workspace in [
        "/other",
        "/project-other",
        "/project/../other",
        "/project/a/../../other",
    ] {
        assert!(CommandProtection::workspace_path(project, Path::new(workspace)).is_err());
    }
}

#[test]
fn commands_use_persistent_compiler_cache_and_private_temporary_storage() {
    let source = std::process::Command::new("cargo");
    let first = GuestCommand::new(&source).unwrap();
    let second = GuestCommand::new(&source).unwrap();
    assert_eq!(first.environment, second.environment);
    for (name, value) in [
        ("RUSTC_WRAPPER", "/usr/local/bin/sccache"),
        ("CARGO_INCREMENTAL", "0"),
        ("SCCACHE_DIR", "/cache"),
        ("SCCACHE_SERVER_UDS", "/tmp/sccache.sock"),
        ("TMPDIR", "/tmp"),
        ("CARGO_TARGET_DIR", "/workspace/target/.joe/linux/build"),
    ] {
        assert_eq!(first.environment[name], value);
    }
}

#[test]
fn command_identifiers_must_be_uuids() {
    for identifier in ["../outside", "/tmp/command", "", "command;exit"] {
        let request = serde_json::json!({
            "request": "cancel",
            "id": identifier,
        });
        assert!(serde_json::from_value::<Request>(request).is_err());
    }
}

#[test]
fn frames_enforce_the_size_limit_before_decoding() {
    let mut frame = b"joe-session:{\"event\":\"ready\"}".to_vec();
    frame.resize(Frame::MAX_BYTES, b' ');
    assert!(matches!(Frame::new(&frame).unwrap(), Frame::Ready {}));
    frame.push(b'\n');
    assert!(Frame::new(&frame).is_err());
    assert!(matches!(
        Frame::new(b"Linux boot output\n").unwrap(),
        Frame::BootOutput
    ));
    assert!(Frame::new(&vec![b'x'; Frame::MAX_BYTES + 1]).is_err());
}

#[test]
fn output_frames_preserve_the_command_and_binary_output() {
    let id = uuid::Uuid::new_v4();
    let output = b"\x00\xff\r\n";
    let payload = serde_json::json!({
        "event": "command",
        "id": id,
        "message": {
            "event": "output",
            "stream": "stderr",
            "data": STANDARD.encode(output),
        },
    });
    let frame = format!("joe-session:{payload}\n");
    assert!(matches!(
        Frame::new(frame.as_bytes()).unwrap(),
        Frame::Command {
            id: command_id,
            event: CommandEvent::Output { stream: OutputStream::Stderr, bytes },
        } if command_id == id && bytes == output
    ));
}

#[test]
fn malformed_protocol_frames_are_rejected() {
    let id = uuid::Uuid::new_v4();
    for payload in [
        serde_json::json!({"event": "unknown"}),
        serde_json::json!({"event": "boot_output"}),
        serde_json::json!({"event": "ready", "extra": true}),
        serde_json::json!({"event": "command", "id": "invalid", "message": {"event": "exited", "exit_code": 0}}),
        serde_json::json!({"event": "command", "id": id, "extra": true, "message": {"event": "exited", "exit_code": 0}}),
        serde_json::json!({"event": "command", "id": id, "message": {"event": "exited", "exit_code": 0, "extra": true}}),
        serde_json::json!({"event": "command", "id": id, "message": {"event": "unknown"}}),
        serde_json::json!({"event": "command", "id": id, "message": {"event": "output", "stream": "stdin", "data": ""}}),
        serde_json::json!({"event": "command", "id": id, "message": {"event": "output", "stream": "stdout", "data": "!"}}),
    ] {
        assert!(
            Frame::new(format!("joe-session:{payload}\n").as_bytes()).is_err(),
            "{payload}"
        );
    }
    assert!(Frame::new(b"joe-session:{\n").is_err());
}
