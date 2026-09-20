use crate::actor::{ActorContext, ActorInfo};
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::ToolDef;
use utils::utils::FnvHashMap;
use worker_registry::report::WorkerView;
use worker_registry::request::{WorkerRequest, WorkerRequestInput};

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "start_worker",
    description = "Start a bounded independent worker with no provider request-count limit and return its registered ID immediately. Work directly for small tasks. At most four workers and one writer; child delegation is disabled. Retrieve the final structured report with worker_status before completing the parent turn."
)]
pub struct StartWorker {
    #[tool(input)]
    pub input: WorkerRequestInput,
}

impl std::fmt::Display for StartWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "- start worker: {}", self.input.objective)
    }
}

pub fn info(context: &ActorContext<RustContext>) -> anyhow::Result<&ActorInfo<RustContext>> {
    match context {
        ActorContext::ActorInfo(info) => Ok(info),
        ActorContext::Noop => Err(anyhow::anyhow!(
            "Worker management requires an active actor"
        )),
    }
}

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for StartWorker {
    type Input = WorkerRequestInput;
    type Output = WorkerView;

    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &RustContext,
        actor: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        let info = info(actor)?;
        let request = WorkerRequest::new(input, |name| {
            info.services.tool(name).map(|tool| tool.effect())
        })?;
        crate::worker_registry::launch::start(&info.runtime.workers, info, context, request)
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
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect() -> tools::tool_defs::ToolEffect {
        tools::tool_defs::ToolEffect::DelegateRead
    }
}
