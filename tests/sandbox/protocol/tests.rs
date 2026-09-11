use super::*;

#[test]
fn command_identifiers_cannot_escape_the_temporary_directory() {
    for identifier in ["../outside", "/tmp/command", "", "command;exit"] {
        let configuration = serde_json::json!({
            "firmware": "/runtime/libkrunfw",
            "init": "/runtime/joe-init",
            "rootfs": "/runtime/rootfs",
            "workspace": "/workspace",
            "temporary_name": identifier,
        });
        assert!(serde_json::from_value::<Configuration>(configuration).is_err());
    }
}
