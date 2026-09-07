use crate::{
    actor::Message,
    turn::{ProviderRun, Tag},
};
use clients::{
    failure::{Failure, FailureKind},
    llm::{LLmClient, StreamEvent},
};
use futures::{FutureExt, StreamExt};
use ractor::ActorRef;
use std::{panic::AssertUnwindSafe, time::Duration};
use utils::execution::{ExecutionScope, ResourceKind};

#[derive(Debug)]
pub enum ProviderEvent {
    ContextNotice(String),
    CompactionUsage(common_models::tui_models::TokenCount),
    ContextPrepared {
        update: crate::compactor::ContextUpdate,
        reply: tokio::sync::oneshot::Sender<Result<(), Failure>>,
    },
    Compacted,
    Item(StreamEvent),
    Finished(Result<(), Failure>),
}

#[derive(Clone)]
pub struct ProviderTarget {
    pub(crate) actor: ActorRef<Message>,
    pub(crate) tag: Tag,
}

impl ProviderTarget {
    pub fn send(&self, event: ProviderEvent) -> Result<(), Failure> {
        self.actor
            .send_message(Message::Provider {
                tag: self.tag,
                event,
            })
            .map_err(|_| Failure::new(FailureKind::Worker, "Provider owner stopped"))
    }
}

pub(crate) struct ProviderTask {
    pub target: ProviderTarget,
    pub client: LLmClient,
    pub timeout: Duration,
}

impl ProviderTask {
    pub fn spawn(
        self,
        input: crate::context::ContextInput,
        run: &ProviderRun,
        owner: &ExecutionScope,
        previous: Option<ExecutionScope>,
    ) {
        let scope = run.scope.clone();
        let attempt = run.attempt;
        let task = scope.tasks.clone().spawn(async move {
            let _registration = scope.register(
                ResourceKind::Provider,
                format!("Turn {} request {}", self.target.tag.turn, self.target.tag.operation),
            );
            if let Some(previous) = previous {
                previous.finish().await;
            }
            let target = self.target.clone();
            tokio::select! {
                biased;
                _ = scope.cancel.cancelled() => {},
                result = AssertUnwindSafe(scope.enter(self.pump(input, attempt))).catch_unwind() => {
                    let result = result.unwrap_or_else(|_| Err(Failure::new(FailureKind::Transport, "Provider task panicked")));
                    let _ = target.send(ProviderEvent::Finished(result));
                }
            }
        });
        owner.tasks.spawn(async move {
            let _ = task.await;
        });
    }

    async fn pump(
        mut self,
        input: crate::context::ContextInput,
        attempt: u8,
    ) -> Result<(), Failure> {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
        }
        let prepared = tokio::time::timeout(
            self.timeout,
            crate::compactor::prepare(&input, &mut self),
        ).await
            .map_err(|_| anyhow::anyhow!("Context compaction timed out"))
            .and_then(std::convert::identity)
            .map_err(|error| Failure::new(FailureKind::ContextOverflow, format!("Context preparation failed: {error}. Saved history is intact. Retry /compact, adjust the context limit, or use /new.")))?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        self.target.send(ProviderEvent::ContextPrepared {
            update: prepared.update,
            reply,
        })?;
        tokio::time::timeout(self.timeout, receive)
            .await
            .map_err(|_| Failure::new(FailureKind::Worker, "Context commit timed out"))?
            .map_err(|_| Failure::new(FailureKind::Worker, "Context commit was cancelled"))??;
        match input.mode {
            crate::context::RequestMode::Compact => self.target.send(ProviderEvent::Compacted),
            mode => self.stream(prepared.request, mode).await,
        }
    }

    async fn stream(
        &mut self,
        request: clients::llm::ClientRequest,
        mode: crate::context::RequestMode,
    ) -> Result<(), Failure> {
        let limit_mib = match mode {
            crate::context::RequestMode::SingleResponse => 16,
            _ => 64,
        };
        let mut stream = tokio::time::timeout(self.timeout, self.client.chat_stream(request))
            .await
            .map_err(|_| Failure::new(FailureKind::Transport, "Provider request timed out"))?
            .map_err(Failure::from_error)?;
        let mut state = PumpState::Streaming;
        let mut bytes = 0usize;
        while matches!(state, PumpState::Streaming) {
            state = match tokio::time::timeout(self.timeout, stream.next()).await {
                Ok(Some(Ok(event))) => {
                    bytes = bytes.saturating_add(
                        serde_json::to_vec(&event)
                            .map_err(|error| {
                                Failure::new(FailureKind::InvalidInput, error.to_string())
                            })?
                            .len(),
                    );
                    match bytes <= limit_mib * 1024 * 1024 {
                        true => Ok(()),
                        false => Err(Failure::new(
                            FailureKind::Truncation,
                            format!("Provider response exceeds the {limit_mib} MiB stream limit"),
                        )),
                    }?;
                    match self.target.send(ProviderEvent::Item(event)).is_ok() {
                        true => PumpState::Streaming,
                        false => PumpState::Finished(Ok(())),
                    }
                }
                Ok(Some(Err(error))) => PumpState::Finished(Err(Failure::from_error(error))),
                Ok(None) => PumpState::Finished(Ok(())),
                Err(_) => PumpState::Finished(Err(Failure::new(
                    FailureKind::Transport,
                    "Provider stream timed out",
                ))),
            };
        }
        match state {
            PumpState::Finished(result) => result,
            PumpState::Streaming => Err(Failure::new(
                FailureKind::Transport,
                "Provider stream did not finish",
            )),
        }
    }
}

enum PumpState {
    Streaming,
    Finished(Result<(), Failure>),
}
