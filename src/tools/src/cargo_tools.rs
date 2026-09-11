use crate::tool_defs::{
    CancellationMode, LenientDeserialize, NonEmptyString, ToolDefTrait, ToolEffect, ToolId,
    ToolProperty, ToolTrait, ToolType,
};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fmt::{Display, Formatter},
    marker::PhantomData,
};
use utils::{
    cargo::{CargoAction, CargoInput, CargoOperation, CargoResult, OutputOffsets, ProcessAction},
    utils::FnvHashMap,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
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

    fn effect(&self) -> ToolEffect {
        match self {
            Self::Fmt(_) | Self::Run(_) | Self::Start(_) => ToolEffect::Write,
            Self::Poll(_) | Self::Stop(_) => ToolEffect::ProcessControl,
            _ => ToolEffect::Validate,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessInput {
    pub process_id: NonEmptyString,
    #[serde(default)]
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
}

pub struct Cargo<P: CargoPolicy = AllOperations>(PhantomData<P>);
impl<P: CargoPolicy> Display for Cargo<P> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("cargo")
    }
}

impl<P: CargoPolicy> ToolDefTrait for Cargo<P> {
    fn tool_name() -> &'static str {
        "cargo"
    }
    fn tool_description() -> &'static str {
        "Run typed Rust operations in the project sandbox. Missing crates.io dependencies are downloaded automatically. Select operation: check, test, fmt, fmt_check, clippy, run, start, poll or stop, as available to this worker. Prefer targeted validation; compilation alone does not prove behavior. run/start require a named binary or example; start returns a process_id for poll/stop. Stop managed targets before edits or other Cargo operations. Processes stop with the turn or after five minutes. Returns structured command, diagnostics, status and output evidence."
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        properties(P::OPERATIONS)
    }

    fn required_fields() -> Vec<String> {
        vec!["operation".into()]
    }
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
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
        CargoInvocation::new(input, P::OPERATIONS)?.execute().await
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
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect_from_input(input: &Self::Input) -> ToolEffect {
        input.effect()
    }
}

fn properties(operations: &[&str]) -> FnvHashMap<String, ToolProperty> {
    json!({
        "operation": {"type":"string", "enum":operations, "description":"Required operation. Test filters require test; deny_warnings requires clippy. fmt/fmt_check accept workspace/package selection. run/start require a named bin/example and accept literal args. poll/stop accept only process_id and offsets."},
        "workspace": {"type":"boolean", "description":"Select the workspace; mutually exclusive with package. Not available for run/start."},
        "package": {"type":"string", "description":"One Cargo package name; omit to use Cargo's default members.", "minLength":1, "maxLength":256},
        "environment": {"type":"object", "description":"Clean environment additions: RUST_LOG, RUST_BACKTRACE, NO_COLOR, or uppercase JOE_RUN_* keys. No toolchain, loader, Cargo or network overrides.", "additionalProperties":{"type":"string","maxLength":4096}, "maxProperties":16},
        "timeout_seconds":{"type":"integer","minimum":1,"maximum":300,"description":"Execution deadline including build time; defaults to 300 seconds."},
        "features":{"type":"array","items":{"type":"string","minLength":1,"maxLength":256},"maxItems":64},
        "all_features":{"type":"boolean"},
        "no_default_features":{"type":"boolean"},
        "target":{"oneOf":[named_target("bin"),named_target("example"),named_target("test"),{"type":"object","properties":{"kind":{"type":"string","enum":["lib","all","tests"]}},"required":["kind"],"additionalProperties":false}]},
        "target_triple":{"type":"string","description":"Built-in Rust target triple; no custom target JSON or paths."},
        "release":{"type":"boolean"},
        "include_warnings":{"type":"boolean","description":"Accepted for compatibility; structured diagnostics always retain warnings."},
        "test_name":{"type":"string","description":"Optional test name filter for the test operation."},
        "exact":{"type":"boolean","description":"Require an exact test_name match."},
        "show_output":{"type":"boolean","description":"Include output from successful tests."},
        "deny_warnings":{"type":"boolean","description":"Deny warnings during clippy."},
        "args":{"type":"array","items":{"type":"string","maxLength":4096},"maxItems":64,"description":"For run/start: individual literal program arguments after --. No shell expansion."},
        "process_id":{"type":"string","minLength":1,"description":"Required for poll/stop: opaque ID returned by operation start in this turn."},
        "offsets":{"type":"object","properties":{"stdout":{"type":"integer","minimum":0},"stderr":{"type":"integer","minimum":0}},"additionalProperties":false,"description":"For poll/stop: use previous next_offset values for incremental output; omit for all retained output."}
    })
    .as_object().unwrap().iter()
    .filter(|(name, _)| match operations {
        ["fmt"] => matches!(name.as_str(), "operation" | "workspace" | "package" | "environment" | "timeout_seconds"),
        _ => true,
    })
    .map(|(name, schema)| (name.clone(), ToolProperty::Schema(schema.clone())))
    .collect()
}

fn named_target(kind: &str) -> Value {
    json!({"type":"object","properties":{"kind":{"type":"string","enum":[kind]},"name":{"type":"string","minLength":1,"maxLength":256}},"required":["kind","name"],"additionalProperties":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_defs::erased_tool;
    use analysis::contexts::rust_empty_context::RustEmptyContext;

    #[test]
    fn preparation_rejects_missing_operations_and_incompatible_inputs() {
        let cargo = erased_tool::<Cargo, RustEmptyContext, ()>();
        for input in [
            json!({}),
            json!({"operation":"shell"}),
            json!({"operation":"check","command":"shell"}),
            json!({"operation":"check","package":"--offline=false"}),
            json!({"operation":"check","package":"member","workspace":true}),
            json!({"operation":"check","test_name":"filter"}),
            json!({"operation":"check","target":{"kind":"bin","name":"--config=bad"}}),
            json!({"operation":"check","target":{"kind":"unknown"}}),
            json!({"operation":"check","features":"all"}),
            json!({"operation":"fmt","features":["gated"]}),
            json!({"operation":"start"}),
            json!({"operation":"run","target":{"kind":"lib"}}),
            json!({"operation":"poll"}),
            json!({"operation":"stop","process_id":""}),
            json!({"operation":"poll","process_id":"id","package":"member"}),
            json!({"operation":"stop","process_id":"id","offsets":{"stdout":-1}}),
            json!({"operation":"check","process_id":"id"}),
        ] {
            assert!(cargo.display_erased(&input).is_err(), "{input}");
        }
        assert!(cargo.display_erased(&json!({"operation":"check"})).is_ok());
        assert!(
            cargo
                .display_erased(&json!({"operation":"test","test_name":"regression","exact":true}))
                .is_ok()
        );
        assert!(
            cargo
                .display_erased(
                    &json!({"operation":"start","target":{"kind":"example","name":"server"}})
                )
                .is_ok()
        );
        assert!(
            cargo
                .display_erased(
                    &json!({"operation":"poll","process_id":"id","offsets":{"stdout":2}})
                )
                .is_ok()
        );
    }

    #[test]
    fn worker_permissions_reject_operations_outside_the_advertised_set() {
        let validation = erased_tool::<Cargo<ValidationOperations>, RustEmptyContext, ()>();
        let formatting = erased_tool::<Cargo<FormattingOperations>, RustEmptyContext, ()>();
        assert!(
            validation
                .display_erased(&json!({"operation":"fmt"}))
                .is_err()
        );
        assert!(
            validation
                .display_erased(&json!({"operation":"fmt_check"}))
                .is_ok()
        );
        assert!(
            formatting
                .display_erased(&json!({"operation":"fmt"}))
                .is_ok()
        );
        assert!(
            formatting
                .display_erased(&json!({"operation":"check"}))
                .is_err()
        );
        assert!(
            formatting
                .display_erased(
                    &json!({"operation":"run","target":{"kind":"example","name":"server"}})
                )
                .is_err()
        );
    }
}
