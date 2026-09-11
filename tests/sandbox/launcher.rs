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
