use crate::actor::Message;
use crate::compactor::{CompactedContext, ContextUpdate};
use crate::event_reporter::EventReporter;
use crate::immutable_workers::{ImmutableWorker, ImmutableWorkerDescription};
use crate::states::runtime::Runtime;
use crate::states::stream_processor::ProviderAction;
use crate::workers::snapshot_worker::SnapshotWorker;
use clients::failure::{Failure, FailureKind};
use clients::response::StreamNextStep;
use common_models::tui_models::{ActorToTuiPacket, TokenCount};
use ractor::ActorRef;
use session::state::SessionState;
use turn_engine::machine::ProviderUpdate;
use turn_engine::turn::Tag;

pub struct ProviderSession<'a> {
    pub session: &'a mut SessionState,
    pub runtime: &'a Runtime,
    pub reporter: &'a EventReporter,
    pub actor: &'a ActorRef<Message>,
    pub actor_id: u64,
    pub usage: TokenCount,
}

impl ProviderSession<'_> {
    pub async fn apply(mut self, tag: Tag, action: ProviderAction) -> ProviderUpdate {
        match action {
            ProviderAction::Update(update) => {
                match &update {
                    ProviderUpdate::Finished(Err(failure)) => {
                        tracing::warn!(
                            turn_id = %tag.turn,
                            operation_id = %tag.operation,
                            error = %failure,
                            "Provider request failed"
                        );
                        self.record_usage();
                    }
                    ProviderUpdate::Finished(Ok(_)) | ProviderUpdate::ReconcilePlan { .. } => {
                        self.record_usage();
                    }
                    ProviderUpdate::Progress(_) => {}
                }
                update
            }
            ProviderAction::Usage(usage) => {
                self.session
                    .control(self.runtime.session_access(self.reporter))
                    .persistence
                    .record(session::Event::Usage(usage.clone()));
                self.reporter.send(ActorToTuiPacket::TokensUpdated(usage));
                ProviderUpdate::Progress(StreamNextStep::Noop)
            }
            ProviderAction::Commit { update, reply } => {
                let _ = reply.send(self.commit(update).await);
                ProviderUpdate::Progress(StreamNextStep::Noop)
            }
        }
    }

    fn record_usage(&mut self) {
        self.session
            .control(self.runtime.session_access(self.reporter))
            .persistence
            .record(session::Event::Usage(self.usage.clone()));
    }

    async fn commit(mut self, update: ContextUpdate) -> Result<(), Failure> {
        match update.compaction {
            Some(compaction) => self.compact(*compaction).await,
            None => Ok(()),
        }?;
        if let Some(message) = self.session.persistence.committed(update.runtime_update)? {
            self.session
                .control(self.runtime.session_access(self.reporter))
                .append_history(vec![message]);
        }
        let request = self.session.persistence.committed(update.request)?;
        self.reporter
            .send(ActorToTuiPacket::ContextUpdated(request));
        Ok(())
    }

    async fn compact(&mut self, compaction: CompactedContext) -> Result<(), Failure> {
        let worker = ImmutableWorker::spawn(
            SnapshotWorker,
            compaction.snapshot,
            ImmutableWorkerDescription {
                kind: "snapshot".into(),
                description: format!(
                    "Older context preserved by compaction generation {}. Includes the compacted exchanges and any earlier compaction memory.",
                    compaction.checkpoint.generation
                ),
            },
            self.actor,
        )
        .await
        .map_err(|error| Failure::new(FailureKind::Worker, error.to_string()))?;
        self.session
            .control(self.runtime.session_access(self.reporter))
            .persistence
            .record(session::Event::Compacted {
                context: compaction.checkpoint.clone(),
                usage: self.usage.clone(),
            });
        self.session
            .conversation
            .commit_checkpoint(self.session.persistence.committed(compaction.checkpoint)?);
        let view = self
            .runtime
            .immutable_workers
            .insert(&self.runtime.worker_owner(self.actor_id), worker);
        self.reporter.send(ActorToTuiPacket::ContextNotice(
            format!("Context compacted. Immutable worker {} preserves the older context; use ask_immutable_worker to query it. The saved transcript and full output artifacts remain available.", view.worker_id),
        ));
        Ok(())
    }
}
