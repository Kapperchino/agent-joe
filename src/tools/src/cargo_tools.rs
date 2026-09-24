use crate::tool_defs::{
    CancellationMode, LenientDeserialize, NonEmptyString, ToolDefTrait, ToolId, ToolOpKind,
    ToolTrait, ToolType,
};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::{Display, Formatter},
    marker::PhantomData,
    time::Duration,
};
use turbo_code_macros::{ToolDef, ToolSchema};
use utils::{
    cargo::{CargoAction, CargoInput, CargoOperation, CargoResult, OutputOffsets, ProcessAction},
    utils::FnvHashMap,
};

#[derive(Clone, Serialize, Deserialize, ToolSchema)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[tool(
    description = "Required operation. Test filters require test; deny_warnings requires clippy. fmt/fmt_check accept workspace/package selection. run/start require a named bin/example and accept literal args. poll/stop accept only process_id and offsets."
)]
pub enum CargoRequest {
    Check(CargoInput),
    Test(CargoInput),
    Fmt(CargoInput),
    FmtCheck(CargoInput),
    Clippy(CargoInput),
    Run(CargoInput),
    Start(CargoInput),
    Poll(ProcessInput),
    Stop(ProcessInput),
}

impl LenientDeserialize for CargoRequest {
    fn deserialize_lenient(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

impl CargoRequest {
    pub fn validation_command(&self) -> anyhow::Result<utils::cargo::CargoCommand> {
        match CargoInvocation::new(
            self.clone(),
            &["check", "test", "fmt_check", "clippy", "run"],
        )? {
            CargoInvocation::Execute(operation) => Ok(operation.details().clone()),
            _ => Err(anyhow::anyhow!(
                "Validation requires a finite Cargo operation"
            )),
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            Self::Check(_) => "check",
            Self::Test(_) => "test",
            Self::Fmt(_) => "fmt",
            Self::FmtCheck(_) => "fmt_check",
            Self::Clippy(_) => "clippy",
            Self::Run(_) => "run",
            Self::Start(_) => "start",
            Self::Poll(_) => "poll",
            Self::Stop(_) => "stop",
        }
    }

    fn effect(&self) -> ToolOpKind {
        match self {
            Self::Fmt(_) | Self::Run(_) | Self::Start(_) => ToolOpKind::Write,
            Self::Poll(_) | Self::Stop(_) => ToolOpKind::ProcessControl,
            _ => ToolOpKind::Validate,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, ToolSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessInput {
    #[tool(
        schema = "String",
        min_length = 1,
        description = "Required for poll/stop: opaque ID returned by operation start in this turn."
    )]
    pub process_id: NonEmptyString,
    #[serde(default)]
    #[tool(
        description = "For poll/stop: use previous next_offset values for incremental output; omit for all retained output."
    )]
    pub offsets: OutputOffsets,
}

enum CargoInvocation {
    Execute(CargoOperation),
    Start(CargoOperation),
    Control {
        action: ProcessAction,
        input: ProcessInput,
    },
}

impl CargoInvocation {
    fn new(request: CargoRequest, allowed: &[&str]) -> anyhow::Result<Self> {
        let name = request.operation();
        let request = allowed.contains(&name).then_some(request).ok_or_else(|| {
            anyhow::anyhow!("Cargo operation {name} is unavailable to this worker")
        })?;
        match request {
            CargoRequest::Check(input) => {
                CargoOperation::new(CargoAction::Check, input).map(Self::Execute)
            }
            CargoRequest::Test(input) => {
                CargoOperation::new(CargoAction::Test, input).map(Self::Execute)
            }
            CargoRequest::Fmt(input) => {
                CargoOperation::new(CargoAction::Format, input).map(Self::Execute)
            }
            CargoRequest::FmtCheck(input) => {
                CargoOperation::new(CargoAction::CheckFormat, input).map(Self::Execute)
            }
            CargoRequest::Clippy(input) => {
                CargoOperation::new(CargoAction::Clippy, input).map(Self::Execute)
            }
            CargoRequest::Run(input) => {
                CargoOperation::new(CargoAction::Run, input).map(Self::Execute)
            }
            CargoRequest::Start(input) => {
                CargoOperation::new(CargoAction::Run, input).map(Self::Start)
            }
            CargoRequest::Poll(input) => Ok(Self::Control {
                action: ProcessAction::Poll,
                input,
            }),
            CargoRequest::Stop(input) => Ok(Self::Control {
                action: ProcessAction::Stop,
                input,
            }),
        }
    }

    fn display(&self) -> anyhow::Result<String> {
        match self {
            Self::Execute(operation) => Ok(format!(
                "cargo {}",
                serde_json::to_string(operation.details())?
            )),
            Self::Start(operation) => Ok(format!(
                "cargo start {}",
                serde_json::to_string(operation.details())?
            )),
            Self::Control {
                action: ProcessAction::Poll,
                input,
            } => Ok(format!("cargo poll {}", input.process_id)),
            Self::Control {
                action: ProcessAction::Stop,
                input,
            } => Ok(format!("cargo stop {}", input.process_id)),
        }
    }

    async fn execute(self) -> anyhow::Result<CargoResult> {
        match self {
            Self::Execute(operation) => operation.execute().await,
            Self::Start(operation) => operation.start().await,
            Self::Control { action, input } => {
                CargoResult::control(input.process_id.as_ref(), input.offsets, action).await
            }
        }
    }
}

pub trait CargoPolicy: Send + Sync {
    const OPERATIONS: &'static [&'static str];
    const FIELDS: &'static [&'static str] = &[];
}

pub struct AllOperations;
impl CargoPolicy for AllOperations {
    const OPERATIONS: &'static [&'static str] = &[
        "check",
        "test",
        "fmt",
        "fmt_check",
        "clippy",
        "run",
        "start",
        "poll",
        "stop",
    ];
}

pub struct ValidationOperations;
impl CargoPolicy for ValidationOperations {
    const OPERATIONS: &'static [&'static str] = &[
        "check",
        "test",
        "fmt_check",
        "clippy",
        "run",
        "start",
        "poll",
        "stop",
    ];
}

pub struct FormattingOperations;
impl CargoPolicy for FormattingOperations {
    const OPERATIONS: &'static [&'static str] = &["fmt"];
    const FIELDS: &'static [&'static str] = &[
        "operation",
        "workspace",
        "package",
        "environment",
        "timeout_seconds",
    ];
}

#[derive(ToolDef)]
#[tool(
    name = "cargo",
    description = "Run typed Rust operations in the project sandbox. Missing crates.io dependencies are downloaded automatically. Select operation: check, test, fmt, fmt_check, clippy, run, start, poll or stop, as available to this worker. Prefer targeted validation; compilation alone does not prove behavior. run/start require a named binary or example; start returns a process_id for poll/stop. Stop managed targets before edits or other Cargo operations. Processes stop with the turn or at timeout_seconds: 30 minutes by default, at most one hour. Returns structured command, diagnostics, status and output evidence.",
    input = "CargoRequest",
    variants = "P::OPERATIONS",
    fields = "P::FIELDS"
)]
pub struct Cargo<P: CargoPolicy = AllOperations>(PhantomData<P>);
impl<P: CargoPolicy> Display for Cargo<P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("cargo")
    }
}

#[async_trait]
impl<C: Context, A, P: CargoPolicy> ToolTrait<C, A> for Cargo<P> {
    type Input = CargoRequest;
    type Output = CargoResult;

    async fn run(
        input: Self::Input,
        _tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let validation = input.validation_command().ok();
        let invocation = CargoInvocation::new(input, P::OPERATIONS)?;
        let before = match validation {
            Some(_) => Some(
                utils::files::operation(utils::changes::ChangeTracker::workspace_fingerprint)
                    .await?,
            ),
            None => None,
        };
        let result = invocation.execute().await?;
        if let Some(before) = before {
            let changes = utils::execution::ExecutionScope::current().changes;
            let recorded = result.clone();
            utils::files::operation(move |workspace| {
                changes.record_validation(workspace, &before, &recorded)
            })
            .await?;
        }
        Ok(result)
    }
    fn display_input(input: &Self::Input) -> String {
        <Self as ToolTrait<C, A>>::prepare_input(input)
            .unwrap_or_else(|error| format!("cargo: {error}"))
    }
    fn prepare_input(input: &Self::Input) -> anyhow::Result<String> {
        CargoInvocation::new(input.clone(), P::OPERATIONS)?.display()
    }
    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        CargoInvocation::new(input.clone(), P::OPERATIONS)?;
        Ok(Default::default())
    }
    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn output_is_error(input: &Self::Input, output: &Self::Output) -> bool {
        match input {
            CargoRequest::Stop(_) => false,
            _ => output.is_error(),
        }
    }
    fn cancellation_mode() -> CancellationMode {
        CancellationMode::AwaitCompletion
    }
    fn execution_budget(input: &Self::Input) -> anyhow::Result<Duration> {
        match CargoInvocation::new(input.clone(), P::OPERATIONS)? {
            CargoInvocation::Execute(operation) => Ok(operation.timeout()),
            CargoInvocation::Start(_) | CargoInvocation::Control { .. } => Ok(Duration::ZERO),
        }
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect_from_input(input: &Self::Input) -> ToolOpKind {
        input.effect()
    }
}

#[cfg(test)]
#[path = "../tests/unit/cargo_tools/tests.rs"]
mod tests;
