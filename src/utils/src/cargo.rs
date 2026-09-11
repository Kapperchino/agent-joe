use anyhow::anyhow;
use cargo_metadata::{CompilerMessage, Message, diagnostic::DiagnosticLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::process::Command;

use crate::{
    execution::ExecutionScope,
    process::{ProcessOutput, ProcessStatus},
    sandbox::Sandbox,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoAction {
    Check,
    Test,
    Format,
    CheckFormat,
    Clippy,
    Run,
}

impl CargoAction {
    fn subcommand(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Test => "test",
            Self::Format | Self::CheckFormat => "fmt",
            Self::Clippy => "clippy",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CargoInput {
    pub workspace: bool,
    pub package: Option<String>,
    pub features: Vec<String>,
    pub all_features: bool,
    pub no_default_features: bool,
    pub target: Option<CargoTarget>,
    pub target_triple: Option<String>,
    pub release: bool,
    pub test_name: Option<String>,
    pub exact: bool,
    pub show_output: bool,
    pub deny_warnings: bool,
    pub include_warnings: Option<bool>,
    pub args: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CargoTarget {
    Lib,
    All,
    Tests,
    Test { name: String },
    Example { name: String },
    Bin { name: String },
}

impl CargoTarget {
    fn arguments(self) -> anyhow::Result<Vec<String>> {
        match self {
            Self::Lib => Ok(vec!["--lib".into()]),
            Self::All => Ok(vec!["--all-targets".into()]),
            Self::Tests => Ok(vec!["--tests".into()]),
            Self::Test { name } => Ok(vec!["--test".into(), CargoSelector::new(&name)?.0]),
            Self::Example { name } => Ok(vec!["--example".into(), CargoSelector::new(&name)?.0]),
            Self::Bin { name } => Ok(vec!["--bin".into(), CargoSelector::new(&name)?.0]),
        }
    }
}

struct TargetTriple(String);
impl TargetTriple {
    fn new(value: String) -> anyhow::Result<Self> {
        match value.len() <= 128
            && value.contains('-')
            && !value.starts_with('-')
            && !value.ends_with(".json")
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c))
        {
            true => Ok(Self(value)),
            false => Err(anyhow!(
                "target_triple must be a built-in Rust target name, not a path or option"
            )),
        }
    }
}

struct ProgramArguments(Vec<String>);
impl ProgramArguments {
    fn new(values: Vec<String>) -> anyhow::Result<Self> {
        match values.len() <= 64
            && values
                .iter()
                .all(|arg| arg.len() <= 4096 && !arg.chars().any(char::is_control))
        {
            true => Ok(Self(values)),
            false => Err(anyhow!(
                "At most 64 program arguments of 4096 bytes without control characters are allowed"
            )),
        }
    }
}

pub struct CargoSelector(String);
impl CargoSelector {
    pub fn new(value: &str) -> anyhow::Result<Self> {
        match !value.is_empty()
            && value.len() <= 256
            && !value.starts_with('-')
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-:/".contains(c))
        {
            true => Ok(Self(value.to_owned())),
            false => Err(anyhow!("Invalid Cargo selector: {value:?}")),
        }
    }
}

pub use crate::process::ProcessCommand as CargoCommand;

pub struct ProgramEnvironment(BTreeMap<String, String>);
impl ProgramEnvironment {
    pub fn new(values: BTreeMap<String, String>) -> anyhow::Result<Self> {
        let allowed = values.len() <= 16
            && values.iter().all(|(key, value)| {
                let name = matches!(key.as_str(), "RUST_LOG" | "RUST_BACKTRACE" | "NO_COLOR")
                    || (key.starts_with("JOE_RUN_")
                        && key.len() <= 64
                        && key
                            .bytes()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'));
                name && value.len() <= 4096 && !value.chars().any(char::is_control)
            });
        match allowed {
            true => Ok(Self(values)),
            false => Err(anyhow!(
                "Environment permits only RUST_LOG, RUST_BACKTRACE, NO_COLOR and JOE_RUN_*; at most 16 values of 4096 bytes without control characters"
            )),
        }
    }
}

pub struct CargoOperation {
    command: CargoCommand,
    timeout_seconds: u64,
}

impl CargoOperation {
    pub fn new(action: CargoAction, input: CargoInput) -> anyhow::Result<Self> {
        let formatting = matches!(action, CargoAction::Format | CargoAction::CheckFormat);
        let selected_run = matches!(
            input.target,
            Some(CargoTarget::Example { .. } | CargoTarget::Bin { .. })
        );
        let timeout_seconds = input.timeout_seconds.unwrap_or(300);
        let input = match action {
            _ if input.workspace && input.package.is_some() => {
                Err(anyhow!("Choose workspace or package, not both"))
            }
            _ if input.all_features && !input.features.is_empty() => {
                Err(anyhow!("Choose all_features or named features"))
            }
            _ if !(1..=300).contains(&timeout_seconds) => {
                Err(anyhow!("timeout_seconds must be between 1 and 300"))
            }
            _ if action != CargoAction::Test
                && (input.test_name.is_some() || input.exact || input.show_output) =>
            {
                Err(anyhow!(
                    "Test filters and harness options require the test operation"
                ))
            }
            _ if input.exact && input.test_name.is_none() => {
                Err(anyhow!("exact requires test_name"))
            }
            _ if action != CargoAction::Clippy && input.deny_warnings => {
                Err(anyhow!("deny_warnings requires the clippy operation"))
            }
            _ if action != CargoAction::Run && !input.args.is_empty() => {
                Err(anyhow!("Program arguments require a binary or example run"))
            }
            CargoAction::Run if !selected_run || input.workspace => Err(anyhow!(
                "Run requires one named binary or example and an optional package"
            )),
            _ if formatting
                && (input.target.is_some()
                    || input.target_triple.is_some()
                    || input.release
                    || input.all_features
                    || input.no_default_features
                    || !input.features.is_empty()) =>
            {
                Err(anyhow!(
                    "Formatting accepts workspace/package selection, not build options"
                ))
            }
            _ => Ok(input),
        }?;
        let environment = ProgramEnvironment::new(input.environment)?;
        let package = input
            .package
            .as_deref()
            .map(CargoSelector::new)
            .transpose()?;
        let features = match input.features.len() <= 64 {
            true => input
                .features
                .iter()
                .map(|name| CargoSelector::new(name))
                .collect::<anyhow::Result<Vec<_>>>(),
            false => Err(anyhow!("At most 64 feature names are allowed")),
        }?;
        let triple = input.target_triple.map(TargetTriple::new).transpose()?;
        let target = input
            .target
            .map(CargoTarget::arguments)
            .transpose()?
            .unwrap_or_default();
        let filter = input
            .test_name
            .as_deref()
            .map(CargoSelector::new)
            .transpose()?;
        let flags = match formatting {
            true => vec![
                input.workspace.then_some("--all"),
                (action == CargoAction::CheckFormat).then_some("--check"),
            ],
            false => vec![
                Some("--message-format=json-diagnostic-rendered-ansi"),
                input.workspace.then_some("--workspace"),
                input.release.then_some("--release"),
                input.all_features.then_some("--all-features"),
                input.no_default_features.then_some("--no-default-features"),
            ],
        };
        let trailing = match action {
            CargoAction::Test => [
                (input.exact || input.show_output).then_some("--"),
                input.exact.then_some("--exact"),
                input.show_output.then_some("--show-output"),
            ]
            .into_iter()
            .flatten()
            .map(str::to_owned)
            .collect(),
            CargoAction::Clippy if input.deny_warnings => {
                ["--", "-D", "warnings"].map(str::to_owned).to_vec()
            }
            CargoAction::Run => std::iter::once("--".to_owned())
                .chain(ProgramArguments::new(input.args)?.0)
                .collect(),
            _ => Vec::new(),
        };
        let command = CargoCommand {
            program: "cargo".into(),
            args: std::iter::once(action.subcommand().to_owned())
                .chain(flags.into_iter().flatten().map(str::to_owned))
                .chain(
                    package
                        .into_iter()
                        .flat_map(|package| ["--package".into(), package.0]),
                )
                .chain(
                    features
                        .into_iter()
                        .flat_map(|feature| ["--features".into(), feature.0]),
                )
                .chain(
                    triple
                        .into_iter()
                        .flat_map(|triple| ["--target".into(), triple.0]),
                )
                .chain(target)
                .chain(filter.map(|filter| filter.0))
                .chain(trailing)
                .collect(),
            environment: environment.0,
        };
        match serde_json::to_vec(&command)?.len() <= 2048 {
            true => Ok(Self {
                command,
                timeout_seconds,
            }),
            false => Err(anyhow!(
                "The complete Cargo command and environment must fit within 2048 JSON bytes"
            )),
        }
    }

    pub fn details(&self) -> &CargoCommand {
        &self.command
    }

    pub async fn execute(self) -> anyhow::Result<CargoResult> {
        let command = self.command.clone();
        let timeout = self.timeout_seconds;
        let started = std::time::Instant::now();
        let output = Sandbox::capture(self, timeout)
            .await
            .unwrap_or_else(|error| ProcessOutput {
                status: match ExecutionScope::current().cancel.is_cancelled() {
                    true => ProcessStatus::Cancelled,
                    false => ProcessStatus::Failed,
                },
                exit_code: None,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(format!("{error:#}")),
            });
        CargoResult::new(command, output, None, OutputOffsets::default())
    }

    pub async fn start(self) -> anyhow::Result<CargoResult> {
        let timeout = self.timeout_seconds;
        let id = Sandbox::start(self, timeout).await?;
        CargoResult::control(&id, OutputOffsets::default(), ProcessAction::Poll).await
    }
}

impl crate::sandbox::SandboxOperation for CargoOperation {}
impl crate::sandbox::sealed::Operation for CargoOperation {
    fn into_command(self) -> Command {
        let mut command = Command::new(self.command.program);
        command
            .args(self.command.args)
            .envs(self.command.environment);
        command
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputOffsets {
    pub stdout: usize,
    pub stderr: usize,
}

pub enum ProcessAction {
    Poll,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputArtifact {
    pub id: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputText {
    pub content: String,
    pub offset: usize,
    pub next_offset: usize,
    pub artifact: Option<OutputArtifact>,
}
impl OutputText {
    fn new(content: String, offset: usize) -> anyhow::Result<Self> {
        let next_offset = content.len();
        let content = content
            .get(offset..)
            .ok_or_else(|| anyhow!("Output offset is outside the stream or splits UTF-8"))?
            .to_owned();
        Ok(Self {
            content,
            offset,
            next_offset,
            artifact: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoResult {
    pub command: CargoCommand,
    pub workspace: String,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub diagnostics: Vec<CompilerMessage>,
    pub stdout: OutputText,
    pub stderr: OutputText,
    pub process_id: Option<String>,
    pub error: Option<String>,
    pub reused: bool,
    pub workspace_revision: Option<u64>,
    pub diagnostics_artifact: Option<OutputArtifact>,
}
impl CargoResult {
    fn new(
        command: CargoCommand,
        output: ProcessOutput,
        process_id: Option<String>,
        offsets: OutputOffsets,
    ) -> anyhow::Result<Self> {
        let diagnostics = output
            .stdout
            .lines()
            .filter_map(|line| match serde_json::from_str::<Message>(line) {
                Ok(Message::CompilerMessage(message)) => Some(message),
                _ => None,
            })
            .collect();
        Ok(Self {
            command,
            workspace: ExecutionScope::current()
                .workspace()?
                .root()
                .display()
                .to_string(),
            status: output.status,
            exit_code: output.exit_code,
            duration_ms: output.duration_ms,
            diagnostics,
            stdout: OutputText::new(output.stdout, offsets.stdout)?,
            stderr: OutputText::new(output.stderr, offsets.stderr)?,
            process_id,
            error: output.error,
            reused: false,
            workspace_revision: None,
            diagnostics_artifact: None,
        })
    }

    pub async fn control(
        id: &str,
        offsets: OutputOffsets,
        action: ProcessAction,
    ) -> anyhow::Result<Self> {
        let owner = ExecutionScope::current().process_owner();
        let process = owner.processes.get(id)?;
        match action {
            ProcessAction::Poll => {}
            ProcessAction::Stop => process.stop().await,
        }
        Self::new(
            process.command().clone(),
            process.output(),
            Some(id.to_owned()),
            offsets,
        )
    }

    pub fn is_error(&self) -> bool {
        match self.status {
            ProcessStatus::Running => false,
            ProcessStatus::Exited => self.exit_code != Some(0),
            _ => true,
        }
    }
}

pub struct Cargo;
pub enum CargoCheck {
    CheckPasses {
        warnings: Vec<CompilerMessage>,
    },
    CheckFailed {
        failures: Vec<CompilerMessage>,
        warnings: Vec<CompilerMessage>,
    },
}
pub enum CargoTest {
    TestPasses { output: String },
    TestFailed { output: String },
}
impl Cargo {
    pub async fn cargo_check() -> anyhow::Result<CargoCheck> {
        let result = CargoOperation::new(CargoAction::Check, CargoInput::default())?
            .execute()
            .await?;
        let warnings = result
            .diagnostics
            .iter()
            .filter(|message| message.message.level == DiagnosticLevel::Warning)
            .cloned()
            .collect();
        let failures: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|message| message.message.level == DiagnosticLevel::Error)
            .cloned()
            .collect();
        match result.is_error() {
            false => Ok(CargoCheck::CheckPasses { warnings }),
            true if failures.is_empty() => Err(anyhow!(
                "Cargo check failed: {}\n{}",
                result.stderr.content,
                result.error.unwrap_or_default()
            )),
            true => Ok(CargoCheck::CheckFailed { failures, warnings }),
        }
    }

    pub async fn cargo_test(
        package: Option<&str>,
        test_name: Option<&str>,
    ) -> anyhow::Result<CargoTest> {
        let input = CargoInput {
            package: package.map(str::to_owned),
            test_name: test_name.map(str::to_owned),
            ..Default::default()
        };
        let result = CargoOperation::new(CargoAction::Test, input)?
            .execute()
            .await?;
        let output = format!(
            "{}\n{}\n{}",
            result.stdout.content,
            result.stderr.content,
            result.error.as_deref().unwrap_or_default()
        );
        match result.is_error() {
            false => Ok(CargoTest::TestPasses { output }),
            true => Ok(CargoTest::TestFailed { output }),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/cargo/tests.rs"]
mod tests;
