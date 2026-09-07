use anyhow::Context;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

pub(super) struct Toolchain {
    pub(super) bin: PathBuf,
    pub(super) cargo_home: PathBuf,
    pub(super) rustup_home: PathBuf,
}

impl Toolchain {
    pub(super) fn new() -> anyhow::Result<Self> {
        let home = dirs::home_dir().context("Cannot locate the installed Rust toolchain")?;
        let configured_cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
        let path = std::env::var_os("PATH").unwrap_or_default();
        let bin = configured_cargo_home
            .iter()
            .map(|home| home.join("bin"))
            .chain(std::env::split_paths(&path))
            .chain(std::iter::once(home.join(".cargo/bin")))
            .find(|path| {
                path.is_absolute()
                    && path.join("cargo").metadata().is_ok_and(|metadata| {
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    })
            })
            .context("Cannot locate an installed Cargo executable in CARGO_HOME or PATH")?
            .canonicalize()?;
        let installation = bin.parent().context("Cargo must have a parent directory")?;
        let cargo_home = configured_cargo_home
            .unwrap_or_else(|| installation.to_path_buf())
            .canonicalize()?;
        let rustup_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let default = home.join(".rustup");
                match default.is_dir() {
                    true => default,
                    false => installation.parent().unwrap_or(&home).join(".rustup"),
                }
            })
            .canonicalize()
            .context("Cannot locate the installed Rust toolchains; set RUSTUP_HOME")?;
        Ok(Self {
            bin,
            cargo_home,
            rustup_home,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    enum Location {
        Path,
        Configured,
        WorkspaceCache,
    }

    #[test]
    fn cargo_runs_with_redirected_home_and_configured_toolchain_paths() {
        if crate::test_support::sandbox_available() {
            let toolchain = Toolchain::new().unwrap();
            let directory =
                std::env::temp_dir().join(format!("joe-toolchain-{}", uuid::Uuid::new_v4()));
            let cache = directory.join("cargo");
            let configured = directory.join("configured-cargo");
            for cargo_home in [&cache, &configured] {
                std::fs::create_dir_all(cargo_home).unwrap();
                std::os::unix::fs::symlink(
                    toolchain.cargo_home.join("registry"),
                    cargo_home.join("registry"),
                )
                .unwrap();
            }
            std::os::unix::fs::symlink(&toolchain.bin, configured.join("bin")).unwrap();
            let system_path = "/usr/bin:/bin:/usr/sbin:/sbin";
            let path = std::env::join_paths(
                std::iter::once(toolchain.bin.clone()).chain(std::env::split_paths(system_path)),
            )
            .unwrap();
            for location in [
                Location::Path,
                Location::Configured,
                Location::WorkspaceCache,
            ] {
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "sandbox::tests::cargo_build_scripts_proc_macros_and_tests_cannot_escape",
                        "--nocapture",
                    ])
                    .env("HOME", &directory)
                    .env_remove("CARGO_HOME")
                    .env_remove("RUSTUP_HOME");
                match location {
                    Location::Path => {
                        command.env("PATH", &path);
                    }
                    Location::Configured => {
                        command
                            .env("PATH", system_path)
                            .env("CARGO_HOME", &configured)
                            .env("RUSTUP_HOME", &toolchain.rustup_home);
                    }
                    Location::WorkspaceCache => {
                        command
                            .env("PATH", &path)
                            .env("CARGO_HOME", &cache)
                            .env("RUSTUP_HOME", &toolchain.rustup_home);
                    }
                }
                let result = command.output().unwrap();
                assert!(
                    result.status.success(),
                    "{location:?}:\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
