use super::{IsolatedCommand, TemporaryDirectory};
use crate::workspace::{Access, ProcessWorkspace};
use anyhow::Context;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(super) fn prepare(
    source: Command,
    workspace: &ProcessWorkspace<'_>,
    temporary: TemporaryDirectory,
) -> anyhow::Result<IsolatedCommand> {
    let workspace = workspace.policy();
    let home = dirs::home_dir().context("Cannot locate the installed Rust toolchain")?;
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cargo"))
        .canonicalize()?;
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".rustup"))
        .canonicalize()?;
    let source = source.as_std();
    let executable = if source.get_program() == OsStr::new("cargo") {
        cargo_home.join("bin/cargo")
    } else {
        PathBuf::from(source.get_program())
    };
    if executable.is_absolute() && executable.is_file() {
        let mut command = Command::new("/usr/bin/bwrap");
        command.env_clear().current_dir(workspace.root()).args([
            "--unshare-all",
            "--unshare-user",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--tmpfs",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
        ]);
        for path in [
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/usr/lib64",
            "/usr/libexec",
            "/usr/share",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/etc/ld.so.cache",
            "/etc/alternatives",
        ] {
            if Path::new(path).exists() {
                command.args(["--ro-bind", path, path]);
            }
        }
        for path in [
            cargo_home.join("bin"),
            cargo_home.join("registry"),
            rustup_home.clone(),
        ] {
            if path.exists() {
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
        }
        let resolved_executable = executable.canonicalize()?;
        command
            .arg("--ro-bind")
            .arg(&resolved_executable)
            .arg(&resolved_executable);
        command
            .arg("--bind")
            .arg(workspace.root())
            .arg(workspace.root());
        for path in workspace.process_protected_paths()? {
            if workspace.check(&path, Access::Read).is_err() {
                if path.is_dir() {
                    command
                        .arg("--tmpfs")
                        .arg(&path)
                        .args(["--chmod", "000"])
                        .arg(&path)
                        .arg("--remount-ro")
                        .arg(&path);
                } else {
                    command.arg("--ro-bind").arg("/dev/null").arg(&path);
                }
            } else {
                command.arg("--ro-bind").arg(&path).arg(&path);
            }
        }
        for path in workspace.read_only_roots() {
            command.arg("--ro-bind").arg(path).arg(path);
        }
        let local_cargo = workspace.root().join("target/.joe/cargo");
        workspace.create_parent_dirs(&local_cargo.join("placeholder"))?;
        for directory in ["index", "cache"] {
            let source = cargo_home.join("registry").join(directory);
            if source.exists() {
                workspace.link_process_cache(
                    &source.canonicalize()?,
                    &local_cargo.join("registry").join(directory),
                )?;
            }
        }
        for (key, value) in [
            ("HOME", workspace.root().to_path_buf()),
            ("TMPDIR", temporary.path().to_path_buf()),
            ("CARGO_TARGET_DIR", workspace.root().join("target")),
            ("CARGO_HOME", local_cargo),
            ("RUSTUP_HOME", rustup_home),
        ] {
            command.args(["--setenv", key]).arg(value);
        }
        let mut path = cargo_home.join("bin").into_os_string();
        path.push(":/usr/bin:/bin:/usr/sbin:/sbin");
        command.args(["--setenv", "PATH"]).arg(path);
        for (key, value) in [
            ("LANG", "C"),
            ("CARGO_NET_OFFLINE", "true"),
            ("RUSTUP_AUTO_INSTALL", "0"),
        ] {
            command.args(["--setenv", key, value]);
        }
        for (key, value) in source.get_envs() {
            if let Some(value) = value {
                command.arg("--setenv").arg(key).arg(value);
            }
        }
        let filter = super::super::seccomp::Filter::new()?;
        filter.attach(&mut command);
        command
            .arg("--chdir")
            .arg(workspace.root())
            .arg("--")
            .arg(executable)
            .args(source.get_args());
        Ok(IsolatedCommand {
            command,
            temporary,
            _filter: filter,
        })
    } else {
        Err(anyhow::anyhow!(
            "An installed absolute executable is required: {}",
            executable.display()
        ))
    }
}
