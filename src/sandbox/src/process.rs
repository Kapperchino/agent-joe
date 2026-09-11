use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessCommand {
    pub program: String,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
}

impl ProcessCommand {
    pub fn from_command(command: &Command) -> Self {
        let command = command.as_std();
        Self {
            program: command.get_program().to_string_lossy().into_owned(),
            args: command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            environment: command
                .get_envs()
                .filter_map(|(key, value)| {
                    value.map(|value| {
                        (
                            key.to_string_lossy().into_owned(),
                            value.to_string_lossy().into_owned(),
                        )
                    })
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Exited,
    Cancelled,
    TimedOut,
    OutputLimit,
    Failed,
}

pub(crate) enum ProcessEnd {
    Exited,
    Cancelled,
    TimedOut,
    OutputLimit,
    Failed { error: String },
}

impl ProcessEnd {
    fn status(&self) -> ProcessStatus {
        match self {
            Self::Exited => ProcessStatus::Exited,
            Self::Cancelled => ProcessStatus::Cancelled,
            Self::TimedOut => ProcessStatus::TimedOut,
            Self::OutputLimit => ProcessStatus::OutputLimit,
            Self::Failed { .. } => ProcessStatus::Failed,
        }
    }

    fn error(self) -> Option<String> {
        match self {
            Self::OutputLimit => Some("Process output exceeded its stream limit".into()),
            Self::Failed { error } => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOutput {
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.status == ProcessStatus::Exited && self.exit_code == Some(0)
    }
}

#[derive(Default)]
pub struct ProcessRegistry(Mutex<BTreeMap<String, Arc<ProcessHandle>>>);

impl ProcessRegistry {
    pub fn insert(&self, handle: Arc<ProcessHandle>) -> anyhow::Result<String> {
        let mut entries = self.0.lock().unwrap();
        match entries.len() < 8 {
            true => {
                let id = uuid::Uuid::new_v4().to_string();
                entries.insert(id.clone(), handle);
                Ok(id)
            }
            false => Err(anyhow::anyhow!(
                "The turn has reached its limit of eight managed targets"
            )),
        }
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Arc<ProcessHandle>> {
        self.0
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown process ID in this turn: {id}"))
    }
}

pub struct ProcessHandle {
    pub(crate) cancel: CancellationToken,
    pub(crate) done: CancellationToken,
    pub(crate) command: ProcessCommand,
    started: Instant,
    state: Mutex<ProcessState>,
}

enum ProcessState {
    Running { stdout: Vec<u8>, stderr: Vec<u8> },
    Completed(ProcessOutput),
}

#[derive(Clone, Copy)]
pub(crate) enum OutputStream {
    Stdout,
    Stderr,
}

impl ProcessHandle {
    pub fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn command(&self) -> &ProcessCommand {
        &self.command
    }

    pub fn fail(&self, error: String) {
        self.complete(ProcessEnd::Failed { error }, None);
        self.done.cancel();
    }

    pub fn new(command: ProcessCommand, cancel: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            cancel,
            done: CancellationToken::new(),
            command,
            started: Instant::now(),
            state: Mutex::new(ProcessState::Running {
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
        })
    }

    pub(crate) fn append(&self, stream: OutputStream, bytes: &[u8], limit: usize) -> bool {
        let mut state = self.state.lock().unwrap();
        match &mut *state {
            ProcessState::Running { stdout, stderr } => {
                let output = match stream {
                    OutputStream::Stdout => stdout,
                    OutputStream::Stderr => stderr,
                };
                let accepted = bytes.len().min(limit.saturating_sub(output.len()));
                output.extend_from_slice(&bytes[..accepted]);
                accepted == bytes.len()
            }
            ProcessState::Completed(_) => false,
        }
    }

    pub(crate) fn complete(&self, end: ProcessEnd, exit_code: Option<i32>) {
        let mut state = self.state.lock().unwrap();
        match &*state {
            ProcessState::Running { stdout, stderr } => {
                *state = ProcessState::Completed(ProcessOutput {
                    status: end.status(),
                    exit_code,
                    duration_ms: self.started.elapsed().as_millis() as u64,
                    stdout: String::from_utf8_lossy(stdout).into_owned(),
                    stderr: String::from_utf8_lossy(stderr).into_owned(),
                    error: end.error(),
                });
            }
            ProcessState::Completed(_) => {}
        }
    }

    pub async fn wait(&self) {
        self.done.cancelled().await;
    }

    pub async fn stop(&self) {
        self.cancel.cancel();
        self.wait().await;
    }

    pub fn output(&self) -> ProcessOutput {
        let state = self.state.lock().unwrap();
        match &*state {
            ProcessState::Running { stdout, stderr } => ProcessOutput {
                status: ProcessStatus::Running,
                exit_code: None,
                duration_ms: self.started.elapsed().as_millis() as u64,
                stdout: output_text(stdout, &ProcessStatus::Running),
                stderr: output_text(stderr, &ProcessStatus::Running),
                error: None,
            },
            ProcessState::Completed(output) => output.clone(),
        }
    }
}

fn output_text(bytes: &[u8], status: &ProcessStatus) -> String {
    let trailing = bytes
        .utf8_chunks()
        .last()
        .map(|chunk| chunk.invalid())
        .unwrap_or_default();
    let partial = matches!(status, ProcessStatus::Running)
        && std::str::from_utf8(trailing).is_err_and(|error| error.error_len().is_none());
    let end = match partial {
        true => bytes.len() - trailing.len(),
        false => bytes.len(),
    };
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
#[path = "../tests/unit/process/tests.rs"]
mod tests;
