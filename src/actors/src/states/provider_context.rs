use crate::actor::ActorContext;
use crate::states::provider_task::{ProviderTarget, ProviderTask};
use crate::states::runtime::{ExecutionRole, Runtime};
use crate::states::services::ActorServices;
use crate::states::stream_processor::ProviderAction;
use crate::workers::snapshot_worker::Snapshot;
use analysis::contexts::context::Context;
use clients::failure::{Failure, FailureKind};
use clients::llm::LLmClient;
use clients::response::RequestMode;
use common_models::interaction::{Investigation, PlanReview, StepState, WorkMode};
use common_models::runtime_ids::TurnId;
use conversation::context::ContextInput;
use session::state::SessionState;
use turn_engine::machine::{ProviderUpdate, TurnMachine};
use turn_engine::turn::{AcceptedResponse, ProviderRun};
use utils::execution::ExecutionScope;

pub struct ProviderContext<'a, C: Context> {
    pub context: &'a C,
    pub session: &'a SessionState,
    pub runtime: &'a Runtime,
    pub request_mode: RequestMode,
    pub services: &'a ActorServices<C, ActorContext<C>>,
}

impl<C: Context> ProviderContext<'_, C> {
    pub fn inspect(&self, reporter: crate::event_reporter::EventReporter)
    where
        C: Clone + 'static,
    {
        let context = self.context.clone();
        let scope = self.runtime.scope.clone();
        scope.tasks.clone().spawn(async move {
            tokio::select! {
                _ = scope.cancel.cancelled() => {},
                text = scope.enter(context.inspect_context()) => reporter.send(common_models::tui_models::ActorToTuiPacket::CommandResult(commands::command::Command::PrintContext, text.unwrap_or_else(|error| format!("Context inspection failed: {error}")))),
            }
        });
    }

    pub fn spawn(
        &self,
        client: &LLmClient,
        target: ProviderTarget,
        run: &ProviderRun,
        owner: &ExecutionScope,
        previous: Option<ExecutionScope>,
    ) {
        let client = client.snapshot();
        let input = self.session.persistence.committed(()).and_then(|()| {
            self.input(run.tag.turn, &client).map_err(|error| {
                Failure::new(
                    FailureKind::InvalidInput,
                    format!("Request context configuration failed: {error}"),
                )
            })
        });
        ProviderTask {
            budget: match &self.runtime.role {
                ExecutionRole::Worker { execution, .. } => Some(execution.budget.clone()),
                ExecutionRole::Root | ExecutionRole::Helper => None,
            },
            target,
            client,
            request_timeout: self.runtime.request_timeout,
            compaction_timeout: self.runtime.compaction_timeout,
        }
        .spawn(input, run, owner, previous);
    }

    pub fn input(&self, turn: TurnId, client: &LLmClient) -> anyhow::Result<ContextInput> {
        let interaction = match self.request_mode {
            RequestMode::SingleResponse => None,
            RequestMode::Continue | RequestMode::Compact => Some(
                self.runtime
                    .role
                    .get_guidance(self.runtime.interaction.mode()),
            ),
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
            (true, false) => Snapshot::from_input(
                self.input(TurnId::new(), client)?,
                client,
                self.runtime.request_timeout,
            ),
            _ => Err(anyhow::anyhow!(
                "Finish the active turn before capturing an immutable snapshot"
            )),
        }
    }

    pub async fn review(&self, action: ProviderAction) -> ProviderAction {
        match (action, self.request_mode, &self.runtime.role) {
            (
                ProviderAction::Update(ProviderUpdate::Finished(Ok(AcceptedResponse::Complete(
                    message,
                )))),
                RequestMode::Continue,
                ExecutionRole::Root,
            ) => {
                let obligations = self.completion_obligations().await.unwrap_or_else(|error| {
                    vec![format!("Could not verify completion: {error}. Resolve the blocker before completing.")]
                });
                match obligations.as_slice() {
                    [] => ProviderAction::Update(ProviderUpdate::Finished(Ok(
                        AcceptedResponse::Complete(message),
                    ))),
                    _ => ProviderAction::Update(ProviderUpdate::ReconcilePlan {
                        message,
                        instruction: format!(
                            "Runtime completion review: this turn is still active.\n{}",
                            obligations.join("\n")
                        ),
                    }),
                }
            }
            (action, _, _) => action,
        }
    }

    async fn completion_obligations(&self) -> anyhow::Result<Vec<String>> {
        let planning = self.session.interaction.planning();
        let pending = self
            .runtime
            .workers
            .pending(&self.runtime.worker_owner(self.context.get_id()));
        let mut obligations = Vec::new();
        if planning.review() == PlanReview::Required {
            obligations.push(format!(
                "Requirements changed; reconcile the saved plan with update_plan using revision={} and requirements_revision={}. Reopen completed steps for review, then continue the user's request.",
                planning.plan.revision, planning.requirements_revision,
            ));
        }
        if !pending.is_empty() {
            obligations.push(format!("Collect and assess worker reports with worker_status (wait for running workers): {}", pending.join("; ")));
        }
        match planning.mode {
            WorkMode::Plan => match planning.plan.investigation() {
                Investigation::Missing => obligations.push(
                    "Plan mode requires a completed investigation before presenting a final plan. Use update_plan to add kind=investigation steps, inspect the relevant sources and resolve consequential unknowns, then complete those steps with observed evidence. Keep future implementation steps pending.".into(),
                ),
                Investigation::Unfinished(steps) => obligations.extend(steps.into_iter().map(|step| {
                    format!("Unfinished investigation step {} ({:?}): {}. Continue the investigation and update_plan with observed evidence, or use request_user_input for an unresolved blocker. Future implementation steps may remain pending.", step.id, step.state, step.description)
                })),
                Investigation::Complete => {}
            },
            WorkMode::Implement => obligations.extend(planning.plan.steps.iter().filter(|step| step.state != StepState::Completed).map(|step| {
                format!("Unfinished plan step {} ({:?}): {}. Continue the work and update_plan with observed evidence, or use request_user_input for an unresolved blocker.", step.id, step.state, step.description)
            })),
        }
        match (
            planning.mode,
            pending.is_empty(),
            self.runtime.scope.workspace(),
        ) {
            (WorkMode::Implement, true, Ok(_)) => {
                let commands = planning
                    .plan
                    .steps
                    .iter()
                    .filter_map(|step| step.validation.as_ref())
                    .map(|validation| {
                        serde_json::from_value::<tools::cargo_tools::CargoRequest>(
                            serde_json::Value::Object(validation.cargo.clone()),
                        )?
                        .validation_command()
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let changes = self.runtime.scope.changes.clone();
                obligations.extend(
                    self.runtime
                        .scope
                        .enter(utils::files::operation(move |workspace| {
                            changes.completion_obligations(workspace, &commands)
                        }))
                        .await?,
                );
                let worktree = self
                    .runtime
                    .session
                    .as_ref()
                    .map(|session| session.snapshot())
                    .transpose()?
                    .and_then(|snapshot| snapshot.worktree);
                if worktree.is_some() {
                    let changes = self.runtime.scope.changes.clone();
                    let review = self
                        .runtime
                        .scope
                        .enter(utils::files::operation(move |workspace| {
                            changes.commit_review(workspace)
                        }))
                        .await?;
                    obligations.extend(match review {
                        utils::changes::CommitReview::Missing => Some("Call review_changes with commit_message: a concise imperative subject of at most 72 characters describing the session changes from your existing context. The subject must describe the current reviewed changes before offering a merge.".into()),
                        utils::changes::CommitReview::Unchanged
                        | utils::changes::CommitReview::Described(_) => None,
                    });
                }
            }
            (WorkMode::Implement, true, Err(_))
                if planning
                    .plan
                    .steps
                    .iter()
                    .any(|step| step.validation.is_some()) =>
            {
                obligations.push("Requested validation cannot be verified without a workspace; report the blocker and ask the user how to proceed.".into());
            }
            _ => {}
        }
        Ok(obligations)
    }
}
