use super::*;

struct FixtureWorkspace {
    root: PathBuf,
}

impl Workspace for FixtureWorkspace {
    fn root(&self) -> &Path {
        &self.root
    }

    fn prepare(&self) -> anyhow::Result<crate::workspace::WorkspaceProtection> {
        Ok(crate::workspace::WorkspaceProtection::default())
    }

    fn read(&self, path: &Path) -> anyhow::Result<String> {
        Ok(std::fs::read_to_string(self.root.join(path))?)
    }

    fn create_parent_dirs(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(
            self.root
                .join(path.parent().context("Missing fixture parent")?),
        )?;
        Ok(())
    }

    fn link_process_cache(&self, source: &Path, destination: &Path) -> anyhow::Result<()> {
        self.create_parent_dirs(destination)?;
        std::os::unix::fs::symlink(source, self.root.join(destination))?;
        Ok(())
    }
}

struct RuntimeFixture {
    directory: PathBuf,
    runtime: Runtime,
    workspace: FixtureWorkspace,
}

impl RuntimeFixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!("joe-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let rootfs = directory.join("runtime/rootfs");
        let firmware = directory.join("runtime/lib/libkrunfw");
        let helper = directory.join("runtime/bin/joe-sandbox");
        let project = directory.join("project");
        for path in [
            "usr/local/cargo/bin",
            "usr/local/rustup",
            "usr/local/libexec",
            "workspace",
        ] {
            std::fs::create_dir_all(rootfs.join(path)).unwrap();
        }
        std::fs::create_dir_all(firmware.parent().unwrap()).unwrap();
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::create_dir(&project).unwrap();
        for path in [
            &firmware,
            &firmware.with_file_name("joe-init"),
            &helper,
            &rootfs.join("usr/local/cargo/bin/cargo"),
        ] {
            std::fs::write(path, "fixture").unwrap();
        }
        std::fs::write(
            rootfs.join("usr/local/libexec/joe-guest"),
            include_bytes!("../../../../src/sandbox/guest.sh"),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Runtime::from_paths(rootfs, firmware, helper, &project).unwrap();
        Self {
            directory,
            runtime,
            workspace: FixtureWorkspace { root: project },
        }
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).unwrap();
    }
}

#[test]
fn runtime_requires_matching_guest_and_paths_outside_the_workspace() {
    let fixture = RuntimeFixture::new();
    let runtime = &fixture.runtime;
    assert!(
        Runtime::from_paths(
            runtime.rootfs.clone(),
            runtime.firmware.clone(),
            runtime.helper.clone(),
            &fixture.directory
        )
        .is_err()
    );
    std::fs::write(runtime.rootfs.join("usr/local/libexec/joe-guest"), "stale").unwrap();
    assert!(
        Runtime::from_paths(
            runtime.rootfs.clone(),
            runtime.firmware.clone(),
            runtime.helper.clone(),
            fixture.workspace.root()
        )
        .is_err()
    );
}

#[test]
fn runtime_requires_the_guest_init_program() {
    let fixture = RuntimeFixture::new();
    let runtime = &fixture.runtime;
    std::fs::remove_file(&runtime.init).unwrap();
    assert!(
        Runtime::from_paths(
            runtime.rootfs.clone(),
            runtime.firmware.clone(),
            runtime.helper.clone(),
            fixture.workspace.root()
        )
        .is_err()
    );
}

#[test]
fn guest_environment_is_clean_and_arguments_stay_out_of_the_helper_protocol() {
    let fixture = RuntimeFixture::new();
    let temporary = TemporaryDirectory::new(&fixture.workspace).unwrap();
    let mut source = Command::new("/usr/bin/env");
    source.env("JOE_RUN_VALUE", "'\"$HOME`id`$(id);*");
    let command = fixture
        .runtime
        .prepare(source, &fixture.workspace, &temporary)
        .unwrap();
    let output = std::process::Command::new("/bin/sh")
        .arg(temporary.path().join("command"))
        .env("JOE_SECRET", "not for the guest")
        .output()
        .unwrap();
    assert!(output.status.success());
    let environment = String::from_utf8(output.stdout).unwrap();
    assert!(environment.contains("JOE_RUN_VALUE='\"$HOME`id`$(id);*\n"));
    assert!(environment.contains("HOME=/workspace\n"));
    assert!(!environment.contains("JOE_SECRET"));
    let configuration: crate::protocol::Configuration = serde_json::from_slice(
        command
            .as_std()
            .get_args()
            .next()
            .unwrap()
            .as_encoded_bytes(),
    )
    .unwrap();
    assert_eq!(configuration.workspace, fixture.workspace.root());
    assert_eq!(configuration.init, fixture.runtime.init);
    assert_eq!(configuration.temporary_name, temporary.id());
    let cache = fixture
        .workspace
        .root()
        .join("target/.joe/linux/cargo/registry/index");
    assert_eq!(
        std::fs::read_link(cache).unwrap(),
        Path::new("/usr/local/cargo/registry/index")
    );
}

#[test]
fn guest_arguments_are_literal_and_preserve_empty_values() {
    let values = ["", "a b", "'\"$HOME`id`$(id);*\\", "--", "こんにちは"];
    let arguments = ["/usr/bin/printf", "%s\\n"]
        .into_iter()
        .chain(values)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &guest_command(&arguments)])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", values.join("\n"))
    );
}
