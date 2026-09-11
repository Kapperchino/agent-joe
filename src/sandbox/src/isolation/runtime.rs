use super::provision::platform::Platform;
use super::*;
use std::{
    ffi::OsStr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(super) struct Runtime {
    pub rootfs: PathBuf,
    pub firmware: PathBuf,
    pub init: PathBuf,
    pub helper: PathBuf,
}

impl Runtime {
    pub(super) fn new(
        workspace: &dyn Workspace,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let installation = super::bootstrap::Installation::new(workspace, check)?;
        let filename = Platform::current()?.firmware();
        Self::from_paths(
            installation.rootfs,
            installation.native.join("lib").join(filename),
            installation.native.join("bin/joe-sandbox"),
            workspace.root(),
        )
    }

    fn from_paths(
        rootfs: PathBuf,
        firmware: PathBuf,
        helper: PathBuf,
        workspace: &Path,
    ) -> anyhow::Result<Self> {
        let runtime = Self {
            rootfs,
            init: firmware.with_file_name("joe-init"),
            firmware,
            helper,
        };
        Platform::current()?;
        let valid = runtime.firmware.is_file()
            && runtime.init.is_file()
            && runtime.helper.is_file()
            && runtime.helper.metadata()?.permissions().mode() & 0o111 != 0
            && runtime.rootfs.join("usr/local/cargo/bin/cargo").is_file()
            && runtime.rootfs.join("usr/local/rustup").is_dir()
            && runtime.rootfs.join("workspace").is_dir()
            && std::fs::read(runtime.rootfs.join("usr/local/libexec/joe-guest"))?
                == include_bytes!("../../guest.sh")
            && runtime
                .read_only_paths()?
                .iter()
                .all(|path| !path.starts_with(workspace) && !workspace.starts_with(path));
        match valid {
            true => Ok(runtime),
            false => Err(anyhow::anyhow!(
                "Joe could not prepare a valid libkrun runtime outside the workspace"
            )),
        }
    }

    pub(super) fn read_only_paths(&self) -> anyhow::Result<Vec<&Path>> {
        Ok(vec![
            &self.rootfs,
            self.firmware
                .parent()
                .context("Sandbox firmware must have a directory")?,
            &self.helper,
        ])
    }

    pub(super) fn prepare(
        &self,
        source: Command,
        workspace: &dyn Workspace,
        temporary: &TemporaryDirectory,
    ) -> anyhow::Result<Command> {
        let guest = GuestCommand::new(source.as_std(), temporary.id())?;
        let cache = Path::new("target/.joe/linux");
        for directory in [cache.join("build"), cache.join("cargo")] {
            workspace.create_parent_dirs(&directory.join("placeholder"))?;
        }
        for directory in ["index", "cache"] {
            workspace.link_process_cache(
                &Path::new("/usr/local/cargo/registry").join(directory),
                &cache.join("cargo/registry").join(directory),
            )?;
        }
        std::fs::create_dir(temporary.path().join("guest"))?;
        std::fs::write(temporary.path().join("command"), guest.script())?;
        let configuration = crate::protocol::Configuration {
            firmware: self.firmware.clone(),
            init: self.init.clone(),
            rootfs: self.rootfs.clone(),
            workspace: workspace.root().into(),
            temporary_name: temporary.id(),
        };
        let mut command = Command::new(&self.helper);
        command
            .env_clear()
            .current_dir(workspace.root())
            .arg(serde_json::to_string(&configuration)?);
        Ok(command)
    }
}

struct GuestCommand {
    executable: String,
    arguments: Vec<String>,
    environment: Vec<String>,
    temporary: uuid::Uuid,
}

impl GuestCommand {
    fn new(command: &std::process::Command, temporary: uuid::Uuid) -> anyhow::Result<Self> {
        let executable = match command.get_program() {
            program if program == OsStr::new("cargo") => "/usr/local/cargo/bin/cargo",
            program => program.to_str().context("Guest executable must be UTF-8")?,
        };
        match Path::new(executable).is_absolute() {
            true => Ok(Self {
                executable: executable.into(),
                arguments: command
                    .get_args()
                    .map(|arg| {
                        arg.to_str()
                            .map(str::to_owned)
                            .context("Guest argument must be UTF-8")
                    })
                    .collect::<anyhow::Result<_>>()?,
                environment: command
                    .get_envs()
                    .filter_map(|(key, value)| {
                        value.map(|value| {
                            Ok(format!(
                                "{}={}",
                                key.to_str().context("Environment name must be UTF-8")?,
                                value.to_str().context("Environment value must be UTF-8")?
                            ))
                        })
                    })
                    .collect::<anyhow::Result<_>>()?,
                temporary,
            }),
            false => Err(anyhow::anyhow!(
                "An absolute Linux guest executable is required: {executable}"
            )),
        }
    }

    fn script(&self) -> String {
        let arguments = [
            "/usr/bin/env".to_owned(),
            "-i".into(),
            "HOME=/workspace".into(),
            format!("TMPDIR=/workspace/target/.joe/tmp/{}/guest", self.temporary),
            "CARGO_TARGET_DIR=/workspace/target/.joe/linux/build".into(),
            "CARGO_HOME=/workspace/target/.joe/linux/cargo".into(),
            "RUSTUP_HOME=/usr/local/rustup".into(),
            "PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin".into(),
            "LANG=C".into(),
            "CARGO_NET_OFFLINE=true".into(),
            "RUSTUP_AUTO_INSTALL=0".into(),
        ];
        guest_command(
            &arguments
                .into_iter()
                .chain(self.environment.iter().cloned())
                .chain(std::iter::once(self.executable.clone()))
                .chain(self.arguments.iter().cloned())
                .collect::<Vec<_>>(),
        )
    }
}

fn guest_command(arguments: &[String]) -> String {
    format!(
        "exec {}\n",
        arguments
            .iter()
            .map(|argument| format!("'{}'", argument.replace('\'', "'\\''")))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

#[cfg(test)]
mod tests {
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
            let directory =
                std::env::temp_dir().join(format!("joe-runtime-{}", uuid::Uuid::new_v4()));
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
                include_bytes!("../../guest.sh"),
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
}
