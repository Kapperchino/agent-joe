use crate::actor::ActorContext;
use crate::worker::ContextWorker;
use crate::workers::simple_worker::SimpleWorker;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolOpKind, ToolTrait, ToolType};
use turbo_code_macros::ToolDef;
use utils::utils::FnvHashMap;
use workflows::{Workflow, WorkflowInput, WorkflowReport, WorkflowStatus};

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "run_workflow",
    description = "Run ordered context and agent steps as a pipeline. Built-in agents gather_context, make_changes and validate_rust reuse the existing workflows; define named custom agents for specialized phases. Each agent is a separate bounded worker with inherited constraints and explicit tool/path limits. Prior summaries and shared workspace changes pass to later steps. Stop on worker failure and return all reports; completion does not imply validation passed.",
    input = "WorkflowInput"
)]
pub struct RunWorkflow {
    pub input: WorkflowInput,
}

impl std::fmt::Display for RunWorkflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "- run workflow: {} steps", self.input.steps.len())
    }
}

fn configured(input: &WorkflowInput) -> anyhow::Result<Workflow> {
    let tools = SimpleWorker::<RustContext>::tools();
    Workflow::new(input.clone(), |name| {
        tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(|tool| tool.effect())
    })
}

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for RunWorkflow {
    type Input = WorkflowInput;
    type Output = WorkflowReport;

    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &RustContext,
        actor: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        let info = super::start_worker::info(actor)?;
        let workflow = Workflow::new(input, |name| {
            info.services.tool(name).map(|tool| tool.effect())
        })?;
        super::delegated::run_workflow(workflow, context, actor).await
    }

    fn display_input(input: &Self::Input) -> String {
        Self {
            input: input.clone(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Self {
            input: input.clone(),
        }
        .req()
    }

    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }

    fn output_is_error(_: &Self::Input, output: &Self::Output) -> bool {
        output.status == WorkflowStatus::Stopped
    }

    fn effect() -> ToolOpKind {
        ToolOpKind::DelegateWrite
    }

    fn effect_from_input(input: &Self::Input) -> ToolOpKind {
        configured(input)
            .map(|workflow| workflow.effect())
            .unwrap_or(ToolOpKind::DelegateWrite)
    }

    fn execution_budget(input: &Self::Input) -> anyhow::Result<std::time::Duration> {
        Ok(configured(input)?.execution_budget())
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
