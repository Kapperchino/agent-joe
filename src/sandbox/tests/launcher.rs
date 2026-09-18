#[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
#[path = "../src/configuration.rs"]
mod configuration;

#[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn built_launcher_reports_its_protocol_without_starting_a_vm() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_joe-sandbox"))
        .arg("--protocol-version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        configuration::LAUNCHER_PROTOCOL_VERSION
    );
    assert!(output.stderr.is_empty());
}

#[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn built_launcher_accepts_the_cache_configuration_field() {
    let directory = std::env::temp_dir().join(format!("joe-launcher-{}", uuid::Uuid::new_v4()));
    let configuration = configuration::Configuration {
        firmware: directory.join("firmware"),
        init: directory.join("init"),
        rootfs: directory.join("rootfs"),
        workspace: directory.join("workspace"),
        cache: directory.join("cache"),
    };
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_joe-sandbox"))
        .arg(serde_json::to_string(&configuration).unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Cannot load Joe's bundled libkrun firmware")
    );
}

#[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn built_launcher_rejects_invalid_configuration_before_starting_a_vm() {
    for arguments in [vec![], vec!["{}"], vec!["{}", "{}"]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_joe-sandbox"))
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("configuration"));
    }
}
