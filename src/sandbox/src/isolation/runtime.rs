use super::provision::platform::Platform;
use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(crate) struct Runtime {
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
            && std::fs::read(runtime.rootfs.join("usr/local/libexec/joe-session.py"))?
                == include_bytes!("../../guest.py")
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

    pub(super) fn prepare(&self, workspace: &dyn Workspace) -> anyhow::Result<Command> {
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
        let configuration = crate::configuration::Configuration {
            firmware: self.firmware.clone(),
            init: self.init.clone(),
            rootfs: self.rootfs.clone(),
            workspace: workspace.root().into(),
        };
        let mut command = Command::new(&self.helper);
        command
            .env_clear()
            .current_dir(workspace.root())
            .arg(serde_json::to_string(&configuration)?);
        Ok(command)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/isolation/runtime/tests.rs"]
mod tests;
