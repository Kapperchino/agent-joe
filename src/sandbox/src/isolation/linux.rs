use super::runtime::Runtime;
use crate::workspace::{Workspace, WorkspaceProtection};
use std::path::Path;
use tokio::process::Command;

enum Protection {
    HiddenDirectory,
    HiddenFile,
}

pub(super) fn prepare(
    source: Command,
    workspace: &dyn Workspace,
    protection: &WorkspaceProtection,
    runtime: &Runtime,
    filter: &super::seccomp::Filter,
) -> anyhow::Result<Command> {
    let source = source.as_std();
    let mut command = Command::new(runtime.helper.with_file_name("bwrap"));
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
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/etc/ld.so.cache",
        "/sys/devices/system/cpu",
    ]
    .into_iter()
    .filter(|path| Path::new(path).exists())
    {
        command.args(["--ro-bind", path, path]);
    }
    for path in runtime.read_only_paths()? {
        command.arg("--ro-bind").arg(path).arg(path);
    }
    command.args(["--dev-bind", "/dev/kvm", "/dev/kvm"]);
    command
        .arg("--bind")
        .arg(workspace.root())
        .arg(workspace.root());
    for path in &protection.hidden {
        let protection = match path.is_dir() {
            true => Protection::HiddenDirectory,
            false => Protection::HiddenFile,
        };
        match protection {
            Protection::HiddenDirectory => {
                command
                    .arg("--tmpfs")
                    .arg(path)
                    .args(["--chmod", "000"])
                    .arg(path)
                    .arg("--remount-ro")
                    .arg(path);
            }
            Protection::HiddenFile => {
                command.arg("--ro-bind").arg("/dev/null").arg(path);
            }
        }
    }

    for path in &protection.read_only {
        command.arg("--ro-bind").arg(path).arg(path);
    }
    filter.attach(&mut command);
    command
        .arg("--chdir")
        .arg(workspace.root())
        .arg("--")
        .arg(source.get_program())
        .args(source.get_args());
    Ok(command)
}
