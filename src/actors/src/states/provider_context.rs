use crate::actor::ActorContext;
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use crate::workers::snapshot_worker::Snapshot;
use analysis::contexts::context::Context;
use clients::failure::{Failure, FailureKind};
use clients::llm::LLmClient;
use clients::response::RequestMode;
use common_models::interaction::PlanReview;
use common_models::runtime_ids::TurnId;
use conversation::context::ContextInput;
use session::state::SessionState;
use turn_engine::machine::{ProviderUpdate, TurnMachine};

pub struct ProviderContext<'a, C: Context> {
    pub context: &'a C,
    pub session: &'a SessionState,
    pub runtime: &'a Runtime,
    pub request_mode: RequestMode,
    pub services: &'a ActorServices<C, ActorContext<C>>,
}

impl<C: Context> ProviderContext<'_, C> {
    pub fn input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => Some(self.runtime.role.get_guidance()),
        };
        let instructions = std::iter::once(self.context.effective_instructions()?)
            .chain(interaction)
            .collect::<Vec<_>>()
            .join("\n");
        let runtime = match self.request_mode {
            RequestMode::SingleResponse => None,
            _ => Some(clients::runtime_update::RuntimeSnapshot {
                planning: match self.runtime.role {
                    ExecutionRole::Root => self.session.interaction.planning().into(),
                    _ => clients::runtime_update::PlanningState {
                        mode: self.runtime.interaction.mode(),
                        ..Default::default()
                    },
                },
                evidence: match self.runtime.role {
                    ExecutionRole::Root => self.session.interaction.planning().evidence.clone(),
                    _ => Default::default(),
                },
                questions: self.session.interaction.questions().pending().to_vec(),
                workers: self
                    .runtime
                    .workers
                    .pending(&self.runtime.worker_owner(self.context.get_id())),
            }),
        };
        Ok(ContextInput {
            runtime,
            prompt_cache_key: Some(self.session.conversation.cache_key().to_owned()),
            purpose: match (&self.runtime.role, self.request_mode) {
                (_, RequestMode::SingleResponse) => clients::llm::RequestPurpose::Compaction,
                (ExecutionRole::Root, _) => clients::llm::RequestPurpose::Conversation,
                _ => clients::llm::RequestPurpose::Worker,
            },
            history: self.session.conversation.history().to_vec(),
            checkpoint: self.session.conversation.checkpoint().clone(),
            instructions,
            tools: self.services.tool_definitions(),
            limits: self
                .runtime
                .context_budget
                .resolve(client.context_window())?,
            native: self.runtime.native_compaction,
            mode: self
                .session
                .conversation
                .request_mode(turn, self.request_mode),
        })
    }

    pub fn capture_snapshot(
        &self,
        turn: &TurnMachine,
        client: &LLmClient,
    ) -> anyhow::Result<Snapshot> {
        match (
            turn.is_idle(),
            self.session.conversation.has_deferred_input(),
        ) {
            (true, false) => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Finish the active turn before capturing an immutable snapshot"
            )),
        }?;
        Snapshot::from_input(
            self.input(TurnId::new(), client)?,
            client,
            self.runtime.request_timeout,
        )
    }

    pub fn review(&self, update: ProviderUpdate) -> ProviderUpdate {
        let pending = self
            .runtime
            .workers
            .pending(&self.runtime.worker_owner(self.context.get_id()));
        let review = match (self.request_mode, &self.runtime.role) {
            (RequestMode::Continue, ExecutionRole::Root) => {
                self.session.interaction.planning().review()
            }
            _ => PlanReview::Current,
        };
        match (update, review) {
            (
                ProviderUpdate::Finished(Ok(turn_engine::turn::AcceptedResponse::Complete(
                    message,
                ))),
                PlanReview::Required,
            ) => ProviderUpdate::ReconcilePlan {
                message,
                instruction: format!(
                    "Runtime plan review: this turn is still active. Requirements changed; reconcile the saved plan with update_plan using revision={} and requirements_revision={} from the current planning state. Reopen completed steps for review, then continue the user's request before completing the turn.",
                    self.session.interaction.planning().plan.revision,
                    self.session.interaction.planning().requirements_revision,
                ),
            },
            (ProviderUpdate::Finished(Ok(turn_engine::turn::AcceptedResponse::Complete(_))), _)
                if !pending.is_empty() =>
            {
                ProviderUpdate::Finished(Err(Failure::new(
                    FailureKind::Worker,
                    format!(
                        "Worker reports have not been collected: {}",
                        pending.join("; ")
                    ),
                )))
            }
            (update, _) => update,
        }
    }
}
