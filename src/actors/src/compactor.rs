use crate::{
    context::{
        BudgetPlan, Checkpoint, ContextInput, ContextLimits, Memory, NativeCompaction,
        estimated_tokens,
    },
    provider_task::{ProviderEvent, ProviderTask},
    workers::compaction_worker::CompactionWorker,
};
use clients::llm::ClientRequest;
use common_models::tui_models::{RequestContext, TokenCount};

#[derive(Debug)]
pub struct ContextUpdate {
    pub(crate) checkpoint: Option<Checkpoint>,
    pub(crate) request: RequestContext,
}

pub(crate) struct PreparedRequest {
    pub request: ClientRequest,
    pub update: ContextUpdate,
}

impl PreparedRequest {
    fn new(
        request: ClientRequest,
        checkpoint: Option<Checkpoint>,
        limits: ContextLimits,
    ) -> anyhow::Result<Self> {
        let estimated_tokens = estimated_tokens(&request)?;
        Ok(Self {
            request,
            update: ContextUpdate {
                checkpoint,
                request: RequestContext {
                    estimated_tokens,
                    ceiling: limits.ceiling(),
                    response_reserve: limits.response(),
                },
            },
        })
    }
}

pub(crate) async fn prepare(
    input: &ContextInput,
    task: &mut ProviderTask,
) -> anyhow::Result<PreparedRequest> {
    match input.plan()? {
        BudgetPlan::Ready(request) => PreparedRequest::new(request, None, input.limits),
        BudgetPlan::Compact(plan) => {
            let method = CompactionMethod::new(input, task)?;
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
            PreparedRequest::new(input.request(&checkpoint)?, Some(checkpoint), input.limits)
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
