use crate::{actor::ActorContext, worker_registry::report::WorkerView};
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolDef)]
#[tool(
    name = "worker_status",
    description = "List workers, retrieve status/results, wait up to 60 seconds, cancel, or clean up a finished worker. Follow-up starts a new bounded worker from a completed worker's selected report and the same tool/path limits. Results include observed edits, validation, failures and artifacts. Active workers are cancelled with the parent turn."
)]
pub struct WorkerStatusTool {
    #[tool(input)]
    pub input: WorkerStatusInput,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct WorkerStatusInput {
    #[tool(
        description = "list, status, wait, cancel, cleanup, or follow_up",
        required
    )]
    pub action: String,
    #[tool(description = "Worker ID; required except for list")]
    pub worker_id: Option<String>,
    #[tool(
        description = "Bounded objective for follow_up; does not change the original constraints or permissions"
    )]
    pub message: Option<String>,
    #[tool(description = "Seconds to wait, 0 to 60; default 30")]
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStatusOutput {
    pub workers: Vec<WorkerView>,
}

enum Action {
    List,
    Status(String),
    Wait { id: String, seconds: u64 },
    Cancel(String),
    Cleanup(String),
    FollowUp { id: String, message: String },
}

impl TryFrom<WorkerStatusInput> for Action {
    type Error = anyhow::Error;
    fn try_from(input: WorkerStatusInput) -> anyhow::Result<Self> {
        let id = || {
            input
                .worker_id
                .clone()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Worker ID is required"))
        };
        match input.action.as_str() {
            "list" => Ok(Self::List),
            "status" => Ok(Self::Status(id()?)),
            "wait" if input.seconds.unwrap_or(30) <= 60 => Ok(Self::Wait {
                id: id()?,
                seconds: input.seconds.unwrap_or(30),
            }),
            "cancel" => Ok(Self::Cancel(id()?)),
            "cleanup" => Ok(Self::Cleanup(id()?)),
            "follow_up" => Ok(Self::FollowUp {
                id: id()?,
                message: input
                    .message
                    .filter(|message| !message.trim().is_empty() && message.len() <= 8192)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Follow-up needs a nonempty objective of at most 8 KiB")
                    })?,
            }),
            _ => Err(anyhow::anyhow!(
                "Unknown worker action or invalid wait duration"
            )),
        }
    }
}

impl std::fmt::Display for WorkerStatusTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "- worker {} {}",
            self.input.action,
            self.input.worker_id.as_deref().unwrap_or("")
        )
    }
}

#[async_trait]
impl ToolTrait<RustContext, ActorContext<RustContext>> for WorkerStatusTool {
    type Input = WorkerStatusInput;
    type Output = WorkerStatusOutput;
    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &RustContext,
        actor: &ActorContext<RustContext>,
    ) -> anyhow::Result<Self::Output> {
        let info = super::start_worker::info(actor)?;
        let registry = &info.dep.runtime.workers;
        let owner = info.dep.worker_owner();
        let workers = match Action::try_from(input)? {
            Action::List => registry.collect(&owner),
            Action::Status(id) => vec![registry.status(&owner, &id)?],
            Action::Wait { id, seconds } => vec![registry.wait(&owner, &id, seconds).await?],
            Action::Cancel(id) => vec![registry.cancel(&owner, &id)?],
            Action::Cleanup(id) => vec![registry.cleanup(&owner, &id)?],
            Action::FollowUp { id, message } => {
                let previous = registry
                    .list(&owner)
                    .into_iter()
                    .find(|view| view.worker_id == id)
                    .ok_or_else(|| anyhow::anyhow!("Unknown worker {id} in this session"))?;
                match previous.report {
                    Some(report) if previous.status.terminal() => {
                        let request = previous.request.follow_up(
                            message,
                            format!(
                                "Selected previous worker report:\n{}",
                                serde_json::to_string(&report)?
                            ),
                            |name| info.dep.tool(name).map(|tool| tool.effect()),
                        )?;
                        vec![registry.start(info, context, request)?]
                    }
                    _ => Err(anyhow::anyhow!(
                        "Wait for the worker to finish before following up; cancel first to change an active task"
                    ))?,
                }
            }
        };
        Ok(WorkerStatusOutput { workers })
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
    fn effect() -> tools::tool_defs::ToolEffect {
        tools::tool_defs::ToolEffect::DelegateRead
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
