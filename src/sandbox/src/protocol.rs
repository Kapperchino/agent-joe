use crate::process::OutputStream;
use crate::workspace::Workspace;
use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestCommand {
    pub program: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Run {
        id: uuid::Uuid,
        command: GuestCommand,
        protection: CommandProtection,
    },
    Cancel {
        id: uuid::Uuid,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandProtection {
    workspace: PathBuf,
    read_only: Vec<PathBuf>,
    hidden: Vec<PathBuf>,
}

impl CommandProtection {
    pub fn new(project: &Path, workspace: &dyn Workspace) -> anyhow::Result<Self> {
        let selected = Self::workspace_path(project, workspace.root())?;
        crate::isolation::runtime::Runtime::prepare_workspace(workspace)?;
        let protection = workspace.prepare()?;
        Ok(Self {
            workspace: selected,
            read_only: Self::guest_paths(workspace.root(), protection.read_only)?,
            hidden: Self::guest_paths(workspace.root(), protection.hidden)?,
        })
    }

    fn workspace_path(project: &Path, workspace: &Path) -> anyhow::Result<PathBuf> {
        let relative = workspace
            .strip_prefix(project)
            .context("Sandbox workspaces must remain within the original project")?;
        match relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            true => Ok(Path::new("/joe-project").join(relative)),
            false => Err(anyhow::anyhow!(
                "Sandbox workspace paths cannot contain traversal"
            )),
        }
    }

    fn guest_paths(root: &Path, paths: Vec<PathBuf>) -> anyhow::Result<Vec<PathBuf>> {
        paths
            .into_iter()
            .map(|path| {
                let relative = path
                    .strip_prefix(root)
                    .context("Protected paths must remain in the workspace")?;
                match relative
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                {
                    true => Ok(Path::new("/workspace").join(relative)),
                    false => Err(anyhow::anyhow!("Protected paths cannot contain traversal")),
                }
            })
            .collect()
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum CommandEvent {
    Output {
        stream: OutputStream,
        #[serde(rename = "data", deserialize_with = "decode_output")]
        bytes: Vec<u8>,
    },
    Exited {
        exit_code: Option<i32>,
    },
    Failed {
        error: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    #[serde(skip)]
    BootOutput,
    Ready {},
    Command {
        id: uuid::Uuid,
        #[serde(rename = "message")]
        event: CommandEvent,
    },
}

impl Frame {
    pub const MAX_BYTES: usize = 65536;

    pub fn new(line: &[u8]) -> anyhow::Result<Self> {
        match line.strip_prefix(b"joe-session:") {
            _ if line.len() > Self::MAX_BYTES => {
                Err(anyhow::anyhow!("Sandbox protocol frame exceeds 64 KiB"))
            }
            Some(payload) => Ok(serde_json::from_slice(payload)?),
            None => Ok(Self::BootOutput),
        }
    }
}

fn decode_output<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<u8>, D::Error> {
    STANDARD
        .decode(String::deserialize(decoder)?)
        .map_err(serde::de::Error::custom)
}

impl GuestCommand {
    pub fn new(command: &std::process::Command) -> anyhow::Result<Self> {
        let executable = match command.get_program() {
            program if program == OsStr::new("cargo") => "/usr/local/cargo/bin/cargo",
            program => program.to_str().context("Guest executable must be UTF-8")?,
        };
        match Path::new(executable).is_absolute() {
            true => Ok(Self {
                program: executable.into(),
                args: command
                    .get_args()
                    .map(|arg| {
                        arg.to_str()
                            .map(str::to_owned)
                            .context("Guest argument must be UTF-8")
                    })
                    .collect::<anyhow::Result<_>>()?,
                environment: [
                    ("HOME", "/workspace".to_owned()),
                    ("TMPDIR", "/tmp".into()),
                    (
                        "CARGO_TARGET_DIR",
                        "/workspace/target/.joe/linux/build".into(),
                    ),
                    ("CARGO_HOME", "/workspace/target/.joe/linux/cargo".into()),
                    ("RUSTUP_HOME", "/usr/local/rustup".into()),
                    (
                        "PATH",
                        "/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin".into(),
                    ),
                    ("LANG", "C".into()),
                    ("CARGO_NET_OFFLINE", "true".into()),
                    ("RUSTUP_AUTO_INSTALL", "0".into()),
                    ("RUSTC_WRAPPER", "/usr/local/bin/sccache".into()),
                    ("CARGO_INCREMENTAL", "0".into()),
                    ("SCCACHE_DIR", "/cache".into()),
                    ("SCCACHE_CACHE_SIZE", "10G".into()),
                    ("SCCACHE_SERVER_UDS", "/tmp/sccache.sock".into()),
                    ("SCCACHE_IDLE_TIMEOUT", "0".into()),
                ]
                .into_iter()
                .map(|(key, value)| Ok((key.to_owned(), value)))
                .chain(command.get_envs().filter_map(|(key, value)| {
                    value.map(|value| {
                        Ok((
                            key.to_str()
                                .context("Environment name must be UTF-8")?
                                .to_owned(),
                            value
                                .to_str()
                                .context("Environment value must be UTF-8")?
                                .to_owned(),
                        ))
                    })
                }))
                .collect::<anyhow::Result<_>>()?,
            }),
            false => Err(anyhow::anyhow!(
                "An absolute Linux guest executable is required: {executable}"
            )),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/protocol/tests.rs"]
mod tests;
