use crate::states::provider_task::{ProviderEvent, ProviderTask};
use crate::workers::compaction_worker::CompactionWorker;
use crate::workers::snapshot_worker::Snapshot;
use clients::llm::ClientRequest;
use common_models::tui_models::{RequestContext, TokenCount};
use conversation::context::{
    BudgetPlan, Checkpoint, ContextInput, ContextLimits, Memory, NativeCompaction, estimated_tokens,
};

#[derive(Debug)]
pub struct CompactedContext {
    pub checkpoint: Checkpoint,
    pub snapshot: Snapshot,
}

#[derive(Debug)]
pub struct ContextUpdate {
    pub compaction: Option<Box<CompactedContext>>,
    pub runtime_update: Option<clients::llm::Message>,
    pub request: RequestContext,
}

pub struct PreparedRequest {
    pub request: ClientRequest,
    pub update: ContextUpdate,
}

impl PreparedRequest {
    fn new(
        request: ClientRequest,
        compaction: Option<CompactedContext>,
        limits: ContextLimits,
        runtime_update: Option<clients::llm::Message>,
    ) -> anyhow::Result<Self> {
        let estimated_tokens = estimated_tokens(&request)?;
        Ok(Self {
            request,
            update: ContextUpdate {
                compaction: compaction.map(Box::new),
                runtime_update,
                request: RequestContext {
                    estimated_tokens,
                    ceiling: limits.ceiling(),
                    response_reserve: limits.response(),
                },
            },
        })
    }
}

pub async fn prepare(
    input: &ContextInput,
    task: &mut ProviderTask,
) -> anyhow::Result<PreparedRequest> {
    match input.plan()? {
        BudgetPlan::Ready(request) => PreparedRequest::new(
            request,
            None,
            input.limits,
            input.runtime_update(&input.checkpoint),
        ),
        BudgetPlan::Compact(plan) => {
            let method = CompactionMethod::new(input, task)?;
            let mut captured = plan.request.clone().with_tools(input.tools.clone());
            captured
                .messages
                .extend(input.runtime.as_ref().map(|runtime| clients::llm::Message {
                    role: clients::llm::Role::User,
                    content: vec![clients::llm::ContentBlock::RuntimeUpdate(
                        clients::runtime_update::RuntimeUpdate::Snapshot(runtime.clone()),
                    )],
                }));
            let snapshot =
                Snapshot::new(captured, &task.client, input.limits, task.request_timeout)?;
            task.target.send(ProviderEvent::ContextNotice(format!(
                "Compacting older context using {}…",
                method.description()
            )))?;
            let memory = match method {
                CompactionMethod::Native => {
                    let response = task.client.compact(plan.request.clone()).await?;
                    task.target
                        .send(ProviderEvent::CompactionUsage(TokenCount {
                            input_tokens: response.usage.input_tokens,
                            output_tokens: response.usage.output_tokens,
                        }))?;
                    Memory::Native(response.output)
                }
                CompactionMethod::Summary => {
                    CompactionWorker::run(task, &plan.request.messages, input.limits).await?
                }
            };
            let checkpoint = input.compacted(&plan, memory)?;
            let runtime_update = input.runtime_update(&checkpoint);
            PreparedRequest::new(
                input.request(&checkpoint)?,
                Some(CompactedContext {
                    checkpoint,
                    snapshot,
                }),
                input.limits,
                runtime_update,
            )
        }
    }
}

enum CompactionMethod {
    Native,
    Summary,
}

impl CompactionMethod {
    fn new(input: &ContextInput, task: &ProviderTask) -> anyhow::Result<Self> {
        match (
            input.native,
            task.client.native_compaction(),
            &input.checkpoint.memory,
        ) {
            (NativeCompaction::Enabled, _, _) | (NativeCompaction::Auto, true, _) => {
                Ok(Self::Native)
            }
            (_, _, Some(Memory::Native(_))) => Err(anyhow::anyhow!(
                "This session contains opaque provider context. Enable native compaction to compact it again; the saved state cannot be summarized as text."
            )),
            _ => Ok(Self::Summary),
        }
    }

    fn description(&self) -> &str {
        match self {
            Self::Native => "provider-native compaction",
            Self::Summary => "a conversation summary",
        }
    }
}
