use crate::tool_defs::{
    CancellationMode, LenientDeserialize, ToolDefTrait, ToolEffect, ToolId, ToolProperty,
    ToolTrait, ToolType,
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

impl LenientDeserialize for CargoInput {
    fn deserialize_lenient(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

pub enum CargoExecution {
    Wait,
    Managed,
}

pub trait CargoToolAction: Send + Sync {
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const ACTION: CargoAction;
    const EXECUTION: CargoExecution;
}

pub struct CargoTool<T: CargoToolAction>(PhantomData<T>);
impl<T: CargoToolAction> Display for CargoTool<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(T::NAME)
    }
}

impl<T: CargoToolAction> ToolDefTrait for CargoTool<T> {
    fn tool_name() -> &'static str {
        T::NAME
    }
    fn tool_description() -> &'static str {
        T::DESCRIPTION
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        properties(T::ACTION)
    }
    fn required_fields() -> Vec<String> {
        match T::ACTION {
            CargoAction::Run => vec!["target".into()],
            _ => vec![],
        }
    }
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
}

#[async_trait]
impl<C: Context, A, T: CargoToolAction> ToolTrait<C, A> for CargoTool<T> {
    type Input = CargoInput;
    type Output = CargoResult;

    async fn run(
        input: Self::Input,
        _tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let operation = CargoOperation::new(T::ACTION, input)?;
        match T::EXECUTION {
            CargoExecution::Managed => operation.start().await,
            CargoExecution::Wait => operation.execute().await,
        }
    }
    fn display_input(input: &Self::Input) -> String {
        <Self as ToolTrait<C, A>>::prepare_input(input)
            .unwrap_or_else(|error| format!("{}: {error}", T::NAME))
    }
    fn prepare_input(input: &Self::Input) -> anyhow::Result<String> {
        let operation = CargoOperation::new(T::ACTION, input.clone())?;
        Ok(format!(
            "{} {}",
            T::NAME,
            serde_json::to_string(operation.details())?
        ))
    }
    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        CargoOperation::new(T::ACTION, input.clone())?;
        Ok(Default::default())
    }
    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn output_is_error(output: &Self::Output) -> bool {
        output.is_error()
    }
    fn cancellation_mode() -> CancellationMode {
        CancellationMode::AwaitCompletion
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect() -> ToolEffect {
        match T::ACTION {
            CargoAction::Format | CargoAction::Run => ToolEffect::Write,
            _ => ToolEffect::Validate,
        }
    }
}

macro_rules! action {
    ($marker:ident, $tool:ident, $name:literal, $action:ident, $execution:ident, $description:literal) => {
        pub struct $marker;
        impl CargoToolAction for $marker {
            const NAME: &'static str = $name;
            const DESCRIPTION: &'static str = $description;
            const ACTION: CargoAction = CargoAction::$action;
            const EXECUTION: CargoExecution = CargoExecution::$execution;
        }
        pub type $tool = CargoTool<$marker>;
    };
}

action!(
    Check,
    CargoCheck,
    "cargo_check",
    Check,
    Wait,
    "Check Rust compilation with typed package, feature and target selection. Returns command, diagnostics, exit status and output; compilation alone does not prove behavior."
);
action!(
    Test,
    CargoTest,
    "cargo_test",
    Test,
    Wait,
    "Run Rust tests offline. Prefer a relevant package, target and test filter before broader tests. Reports exact command and full validation evidence."
);
action!(
    Format,
    CargoFmt,
    "cargo_fmt",
    Format,
    Wait,
    "Format the selected package or workspace with rustfmt. This modifies files; use cargo_fmt_check to check formatting."
);
action!(
    CheckFormat,
    CargoFmtCheck,
    "cargo_fmt_check",
    CheckFormat,
    Wait,
    "Check formatting for the selected package or workspace without applying formatting changes."
);
action!(
    Clippy,
    CargoClippy,
    "cargo_clippy",
    Clippy,
    Wait,
    "Run Clippy with typed build selection and optional deny_warnings. Returns structured compiler diagnostics and stderr."
);
action!(
    Run,
    CargoRun,
    "cargo_run",
    Run,
    Wait,
    "Run one named Rust binary or example in the sandbox for at most five minutes. Arguments after -- are program values; environment keys are restricted. Network remains disabled."
);
action!(
    Start,
    CargoStart,
    "cargo_start",
    Run,
    Managed,
    "Start one managed Rust binary or example. Poll using its process_id, then stop it before editing or running other Cargo commands. Turn completion, interruption and shutdown stop it automatically; maximum lifetime is five minutes. Network remains disabled."
);

fn properties(action: CargoAction) -> FnvHashMap<String, ToolProperty> {
    let formatting = matches!(action, CargoAction::Format | CargoAction::CheckFormat);
    let target = match action {
        CargoAction::Run => json!({"oneOf": [named_target("bin"), named_target("example")]}),
        _ => {
            json!({"oneOf": [named_target("bin"), named_target("example"), named_target("test"), {"type":"object", "properties":{"kind":{"type":"string","enum":["lib","all","tests"]}}, "required":["kind"], "additionalProperties":false}]})
        }
    };
    let mut properties = json!({
        "workspace": {"type":"boolean", "description":"Select the workspace; mutually exclusive with package. Defaults to false."},
        "package": {"type":"string", "description":"One Cargo package name; omit to use Cargo's default members.", "minLength":1, "maxLength":256},
        "environment": {"type":"object", "description":"Clean environment additions: RUST_LOG, RUST_BACKTRACE, NO_COLOR, or uppercase JOE_RUN_* keys. Up to 16 entries, 4096 bytes per value. Toolchain, loader, Cargo and network settings cannot be overridden.", "additionalProperties":{"type":"string","maxLength":4096}, "maxProperties":16},
        "timeout_seconds":{"type":"integer","minimum":1,"maximum":300,"description":"Execution deadline including build time; defaults to 300 seconds."}
    }).as_object().cloned().unwrap();
    if !formatting {
        properties.extend(json!({
            "features":{"type":"array","items":{"type":"string","minLength":1,"maxLength":256},"maxItems":64},
            "all_features":{"type":"boolean"},
            "no_default_features":{"type":"boolean"},
            "target":target,
            "target_triple":{"type":"string","description":"Built-in Rust target triple; no custom target JSON or paths."},
            "release":{"type":"boolean"},
            "include_warnings":{"type":"boolean","description":"Accepted for compatibility; structured diagnostics always retain warnings."}
        }).as_object().cloned().unwrap());
    }
    match action {
        CargoAction::Test => properties.extend(json!({"test_name":{"type":"string","description":"Optional test name filter."},"exact":{"type":"boolean","description":"Require an exact test_name match."},"show_output":{"type":"boolean","description":"Include output from successful tests."}}).as_object().cloned().unwrap()),
        CargoAction::Clippy => properties.extend(json!({"deny_warnings":{"type":"boolean"}}).as_object().cloned().unwrap()),
        CargoAction::Run => {
            properties.remove("workspace");
            properties.extend(json!({"args":{"type":"array","items":{"type":"string","maxLength":4096},"maxItems":64,"description":"Program arguments passed as individual literal values after --. No shell expansion."}}).as_object().cloned().unwrap());
        }
        _ => {},
    }
    properties
        .into_iter()
        .map(|(key, schema)| (key, ToolProperty::Schema(schema)))
        .collect()
}

fn named_target(kind: &str) -> Value {
    json!({"type":"object","properties":{"kind":{"type":"string","enum":[kind]},"name":{"type":"string","minLength":1,"maxLength":256}},"required":["kind","name"],"additionalProperties":false})
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessInput {
    pub process_id: String,
    #[serde(default)]
    pub offsets: OutputOffsets,
}
impl LenientDeserialize for ProcessInput {
    fn deserialize_lenient(value: Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

pub trait ProcessControl: Send + Sync {
    const NAME: &'static str;
    const DESCRIPTION: &'static str;
    const ACTION: ProcessAction;
}
pub struct ProcessTool<T: ProcessControl>(PhantomData<T>);
impl<T: ProcessControl> Display for ProcessTool<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(T::NAME)
    }
}
impl<T: ProcessControl> ToolDefTrait for ProcessTool<T> {
    fn tool_name() -> &'static str {
        T::NAME
    }
    fn tool_description() -> &'static str {
        T::DESCRIPTION
    }
    fn field_properties() -> FnvHashMap<String, ToolProperty> {
        json!({"process_id":{"type":"string","description":"Opaque ID returned by cargo_start in this turn."},"offsets":{"type":"object","properties":{"stdout":{"type":"integer","minimum":0},"stderr":{"type":"integer","minimum":0}},"additionalProperties":false,"description":"Use previous next_offset values for incremental output; omit for all retained output."}})
            .as_object().unwrap().iter().map(|(key, schema)| (key.clone(), ToolProperty::Schema(schema.clone()))).collect()
    }
    fn required_fields() -> Vec<String> {
        vec!["process_id".into()]
    }
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
}
#[async_trait]
impl<C: Context, A, T: ProcessControl> ToolTrait<C, A> for ProcessTool<T> {
    type Input = ProcessInput;
    type Output = CargoResult;
    async fn run(
        input: Self::Input,
        _tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        CargoResult::control(&input.process_id, input.offsets, T::ACTION).await
    }
    fn display_input(input: &Self::Input) -> String {
        format!("{} {}", T::NAME, input.process_id)
    }
    fn req_from_input(_input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn output_is_error(output: &Self::Output) -> bool {
        match T::ACTION {
            ProcessAction::Poll => output.is_error(),
            ProcessAction::Stop => false,
        }
    }
    fn cancellation_mode() -> CancellationMode {
        CancellationMode::AwaitCompletion
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect() -> ToolEffect {
        ToolEffect::ProcessControl
    }
}
pub struct Poll;
impl ProcessControl for Poll {
    const NAME: &'static str = "process_poll";
    const DESCRIPTION: &'static str = "Poll a managed target's status, exit code, duration and incremental stdout/stderr. IDs belong to this turn; resumed sessions do not recreate processes.";
    const ACTION: ProcessAction = ProcessAction::Poll;
}
pub struct Stop;
impl ProcessControl for Stop {
    const NAME: &'static str = "process_stop";
    const DESCRIPTION: &'static str = "Stop and reap a managed target and return its retained output. Safe to repeat for a completed process. Stop running targets before edits or other Cargo commands.";
    const ACTION: ProcessAction = ProcessAction::Stop;
}
pub type ProcessPoll = ProcessTool<Poll>;
pub type ProcessStop = ProcessTool<Stop>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialization_is_strict_and_preparation_rejects_invalid_combinations() {
        for input in [
            json!({"package":"--offline=false"}),
            json!({"package":"member","workspace":true}),
            json!({"test_name":"filter"}),
            json!({"target":{"kind":"bin","name":"--config=bad"}}),
        ] {
            let input = CargoInput::deserialize_lenient(input).unwrap();
            assert!(CargoOperation::new(CargoAction::Check, input).is_err());
        }
        for input in [
            json!({"command":"shell"}),
            json!({"target":{"kind":"unknown"}}),
            json!({"features":"all"}),
        ] {
            assert!(CargoInput::deserialize_lenient(input).is_err());
        }
        assert!(CargoInput::deserialize_lenient(json!({})).is_ok());
    }
}
