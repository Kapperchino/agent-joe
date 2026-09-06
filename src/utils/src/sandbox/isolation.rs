use crate::workspace::WorkspacePolicy;
use tokio::process::Command;

#[cfg(target_os = "linux")]
mod linux;

pub(super) struct IsolatedCommand {
    pub(super) command: Command,
    #[cfg(target_os = "linux")]
    pub(super) _filter: super::seccomp::Filter,
}

impl IsolatedCommand {
    pub(super) fn new(command: Command, workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let command = macos::prepare(command, workspace)?;
            Ok(Self { command })
        }
        #[cfg(target_os = "linux")]
        {
            linux::prepare(command, workspace)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = command;
            let _ = workspace;
            Err(anyhow::anyhow!(
                "Project process isolation is unavailable on this platform; execution is disabled"
            ))
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use anyhow::Context;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};

    struct Profile {
        rules: String,
        parameters: Vec<OsString>,
    }

    struct CargoHome {
        path: PathBuf,
    }

    impl CargoHome {
        fn new(workspace: &WorkspacePolicy, home: &Path) -> anyhow::Result<Self> {
            let path = workspace.root().join("target/.joe/cargo");
            workspace.create_parent_dirs(&path.join("placeholder"))?;
            for directory in ["index", "cache"] {
                let source = home.join(".cargo/registry").join(directory);
                if source.exists() {
                    workspace.link_process_cache(
                        &source.canonicalize()?,
                        &path.join("registry").join(directory),
                    )?;
                }
            }
            Ok(Self { path })
        }
    }

    impl Profile {
        fn new(
            workspace: &WorkspacePolicy,
            executable: &Path,
            home: &Path,
        ) -> anyhow::Result<Self> {
            workspace.validate_process_root()?;
            let mut profile = Self {
                rules: r#"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target self))
(allow sysctl-read)
(allow file-read-metadata)
(allow file-read* (literal "/") (subpath "/usr/bin") (subpath "/usr/lib") (subpath "/usr/libexec") (subpath "/usr/share") (subpath "/bin") (subpath "/sbin") (subpath "/System/Library") (subpath "/System/Cryptexes") (subpath "/System/Volumes/Preboot/Cryptexes") (subpath "/Library/Developer") (subpath "/Library/Apple") (subpath "/private/var/db/dyld") (subpath "/private/preboot/Cryptexes") (subpath "/private/etc/ssl") (literal "/dev/null") (literal "/dev/random") (literal "/dev/urandom"))
(allow file-write-data (literal "/dev/null"))
"#.into(),
                parameters: Vec::new(),
            };
            profile.path(
                "allow",
                "file-read* file-write*",
                "subpath",
                workspace.root(),
            );
            profile.path("allow", "file-read*", "literal", executable);
            for relative in [".rustup", ".cargo/bin", ".cargo/registry"] {
                let path = home.join(relative);
                if path.exists() {
                    profile.path("allow", "file-read*", "subpath", &path.canonicalize()?);
                }
            }
            profile.rules.push_str(r#"(deny file-link)
(deny file-read* file-write* (regex #"(^|/)[.]([tT][uU][rR][bB][oO]-[cC][oO][dD][eE])(/|$)"))
(deny file-write* (regex #"(^|/)[.]([gG][iI][tT]|[aA][gG][eE][nN][tT][sS]|[cC][oO][dD][eE][xX])(/|$)"))
"#);
            for root in workspace.read_only_roots() {
                profile.path("deny", "file-write*", "subpath", root);
            }
            Ok(profile)
        }

        fn path(&mut self, decision: &str, operations: &str, filter: &str, path: &Path) {
            let name = format!("PATH_{}", self.parameters.len());
            let mut parameter = OsString::from(format!("{name}="));
            parameter.push(path);
            self.parameters.push(parameter);
            self.rules.push_str(&format!(
                "({decision} {operations} ({filter} (param \"{name}\")))\n"
            ));
        }
    }

    pub(super) fn prepare(
        command: Command,
        workspace: &WorkspacePolicy,
    ) -> anyhow::Result<Command> {
        let source = command.as_std();
        let home = dirs::home_dir()
            .context("Cannot locate the installed Rust toolchain")?
            .canonicalize()?;
        let executable = if source.get_program() == OsStr::new("cargo") {
            home.join(".cargo/bin/cargo")
        } else {
            PathBuf::from(source.get_program())
        };
        if executable.is_absolute() && executable.is_file() {
            let profile = Profile::new(workspace, &executable.canonicalize()?, &home)?;
            let cargo_home = CargoHome::new(workspace, &home)?;
            let mut isolated = Command::new("/usr/bin/sandbox-exec");
            isolated.env_clear().current_dir(workspace.root());
            for parameter in profile.parameters {
                isolated.arg("-D").arg(parameter);
            }
            isolated.args(["-p", &profile.rules, "/usr/bin/env", "-i"]);
            for (key, value) in [
                ("HOME", workspace.root().to_path_buf()),
                ("TMPDIR", workspace.root().join("target/.joe/tmp")),
                ("CARGO_TARGET_DIR", workspace.root().join("target")),
                ("CARGO_HOME", cargo_home.path),
                ("RUSTUP_HOME", home.join(".rustup")),
            ] {
                let mut assignment = OsString::from(format!("{key}="));
                assignment.push(value);
                isolated.arg(assignment);
            }
            let mut path = OsString::from("PATH=");
            path.push(home.join(".cargo/bin"));
            path.push(":/usr/bin:/bin:/usr/sbin:/sbin");
            isolated
                .arg(path)
                .args(["LANG=C", "CARGO_NET_OFFLINE=true", "RUSTUP_AUTO_INSTALL=0"]);
            for (key, value) in source.get_envs() {
                if let Some(value) = value {
                    let mut assignment = key.to_os_string();
                    assignment.push("=");
                    assignment.push(value);
                    isolated.arg(assignment);
                }
            }
            workspace.create_parent_dirs(Path::new("target/.joe/tmp/placeholder"))?;
            isolated.arg(executable).args(source.get_args());
            Ok(isolated)
        } else {
            Err(anyhow::anyhow!(
                "An installed absolute executable is required: {}",
                executable.display()
            ))
        }
    }
}
