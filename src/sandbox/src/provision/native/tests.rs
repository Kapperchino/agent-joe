use super::*;

struct Fixture {
    directory: PathBuf,
    launcher: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-launcher-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let launcher = directory.join("joe-sandbox");
        fs::write(&launcher, "first build").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            directory,
            launcher,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn installed_applications_and_cargo_tests_find_the_built_launcher() {
    let fixture = Fixture::new();
    for executable in ["turbo-code", "deps/utils-test"] {
        let launcher = Launcher::beside(&fixture.directory.join(executable)).unwrap();
        assert_eq!(launcher.path, fixture.launcher);
    }
}

#[test]
fn missing_or_nonexecutable_launchers_fail_with_actionable_errors() {
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.launcher, fs::Permissions::from_mode(0o600)).unwrap();
    let error = Launcher::new(fixture.launcher.clone()).err().unwrap();
    assert!(error.to_string().contains("not an executable file"));
    assert!(Launcher::new(fixture.directory.clone()).is_err());
    fs::remove_file(&fixture.launcher).unwrap();
    let error = Launcher::new(fixture.launcher.clone()).err().unwrap();
    assert!(error.to_string().contains("cargo build -p sandbox"));
}

#[test]
fn rebuilding_the_launcher_invalidates_its_cached_installation() {
    let fixture = Fixture::new();
    let launcher = Launcher::new(fixture.launcher.clone()).unwrap();
    let first = launcher.version(&|| Ok(())).unwrap();
    assert_eq!(first, launcher.version(&|| Ok(())).unwrap());
    fs::write(&fixture.launcher, "second build").unwrap();
    assert_ne!(first, launcher.version(&|| Ok(())).unwrap());
    assert!(
        launcher
            .version(&|| Err(anyhow::anyhow!("cancelled")))
            .is_err()
    );
}
