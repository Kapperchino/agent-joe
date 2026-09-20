use crate::event_reporter::EventReporter;
use crate::states::provider_task::ProviderEvent;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use analysis::contexts::context::Context;
use clients::failure::{Failure, FailureKind};
use clients::llm::StreamEvent;
use clients::response::StreamNextStep;
use common_models::tui_models::{ActorToTuiPacket, State, TokenCount};
use response_stream::{StreamNotification, StreamProcessor};
use tokio::io::AsyncWriteExt;
use tracing::error;
use turn_engine::machine::ProviderUpdate;
use turn_engine::turn::{AcceptedResponse, ResponseState, Tag};

pub struct ProviderStream {
    pub(crate) processor: StreamProcessor,
    stream_log: Option<tokio::fs::File>,
    reporter: EventReporter,
}

pub enum ProviderAction {
    Update(ProviderUpdate),
    Usage(TokenCount),
    Commit {
        update: crate::compactor::ContextUpdate,
        reply: tokio::sync::oneshot::Sender<Result<(), Failure>>,
    },
}

impl ProviderStream {
    pub fn new<C: Context, A>(
        services: &ActorServices<C, A>,
        runtime: &Runtime,
        reporter: EventReporter,
        usage: TokenCount,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            processor: StreamProcessor {
                batches: Vec::new(),
                token_count: usage,
                cur_state: State::Ready,
            },
            stream_log: Self::stream_log(services, runtime)?,
            reporter,
        })
    }

    pub fn usage(&self) -> TokenCount {
        self.processor.token_count.clone()
    }

    pub fn reset(&mut self, usage: TokenCount) {
        self.processor.clear();
        self.processor.token_count = usage;
    }

    pub fn clear(&mut self) {
        self.processor.clear();
    }

    pub fn change_state(&mut self, state: State) {
        let notification = self.processor.change_state(state);
        self.send(notification);
    }

    fn stream_log<C: Context, A>(
        services: &ActorServices<C, A>,
        runtime: &Runtime,
    ) -> anyhow::Result<Option<tokio::fs::File>> {
        match runtime.role {
            ExecutionRole::Root if services.debug_mode => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let path = std::path::PathBuf::from(format!("./logs/stream_{timestamp}.jsonl"));
                let workspace = runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| runtime.scope.workspace())?;
                let file = workspace.open_append(&path)?;
                Ok(Some(tokio::fs::File::from_std(file)))
            }
            _ => Ok(None),
        }
    }

    pub fn take_completed(&mut self) -> Option<clients::llm::Message> {
        let content = self
            .processor
            .batches
            .last()
            .map(|batch| batch.completed_content())
            .filter(|content| !content.is_empty());
        self.processor.clear();
        content.map(|content| clients::llm::Message {
            role: clients::llm::Role::Assistant,
            content,
        })
    }

    pub async fn event(
        &mut self,
        response: ResponseState,
        tag: Tag,
        event: ProviderEvent,
    ) -> ProviderAction {
        match event {
            ProviderEvent::ContextNotice(message) => {
                self.reporter.send(ActorToTuiPacket::ContextNotice(message));
                ProviderAction::Update(ProviderUpdate::Progress(StreamNextStep::Noop))
            }
            ProviderEvent::CompactionUsage(usage) => {
                self.processor.token_count.input_tokens = self
                    .processor
                    .token_count
                    .input_tokens
                    .saturating_add(usage.input_tokens);
                self.processor.token_count.output_tokens = self
                    .processor
                    .token_count
                    .output_tokens
                    .saturating_add(usage.output_tokens);
                ProviderAction::Usage(self.usage())
            }
            ProviderEvent::ContextPrepared { update, reply } => {
                ProviderAction::Commit { update, reply }
            }
            ProviderEvent::Compacted => {
                ProviderAction::Update(ProviderUpdate::Finished(Ok(AcceptedResponse::Compacted)))
            }
            ProviderEvent::Item(item) => {
                let processed = match response.accept(item) {
                    Ok(item) => self.process(item).await,
                    Err(error) => Err(error),
                };
                ProviderAction::Update(match processed {
                    Ok(step) => ProviderUpdate::Progress(step),
                    Err(error) => ProviderUpdate::Finished(Err(provider_input_error(error))),
                })
            }
            ProviderEvent::Finished(Err(failure)) => {
                ProviderAction::Update(ProviderUpdate::Finished(Err(failure)))
            }
            ProviderEvent::Finished(Ok(())) => ProviderAction::Update(ProviderUpdate::Finished(
                finish_response(response, tag.turn, &mut self.processor),
            )),
        }
    }

    pub async fn process(&mut self, item: StreamEvent) -> anyhow::Result<StreamNextStep> {
        self.log_stream_item(&item).await;
        let update = self.processor.process_stream_event(item);
        for notification in update.notifications {
            self.send(notification);
        }
        update.next
    }

    pub fn send(&self, notification: StreamNotification) {
        match notification {
            StreamNotification::Packet(packet) => self.reporter.send(packet),
            StreamNotification::Usage(usage) => self.reporter.usage(usage),
        }
    }

    async fn log_stream_item(&mut self, item: &StreamEvent) {
        if let Some(file) = self.stream_log.as_mut() {
            let result = async {
                let mut line = serde_json::to_vec(item)?;
                line.push(b'\n');
                file.write_all(&line).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                error!("Failed to write stream log: {error}");
            }
        }
    }
}

pub fn finish_response(
    response: turn_engine::turn::ResponseState,
    turn: common_models::runtime_ids::TurnId,
    processor: &mut StreamProcessor,
) -> Result<turn_engine::turn::AcceptedResponse, clients::failure::Failure> {
    let completion = response.completion()?;
    let items = processor.extract_and_pre_process().map_err(|error| {
        clients::failure::Failure::new(
            clients::failure::FailureKind::InvalidInput,
            error.to_string(),
        )
    })?;
    completion.finish(turn, items)
}

fn provider_input_error(error: anyhow::Error) -> Failure {
    error
        .downcast_ref::<Failure>()
        .cloned()
        .unwrap_or_else(|| Failure::new(FailureKind::InvalidInput, error.to_string()))
}
