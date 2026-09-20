use crate::{
    session::{
        Event,
        interaction_state::{Interaction, InteractionUpdate},
        persistence::Persistence,
        session_merge::MergeDecision,
    },
    states::{
        actor_state::ActorState, runtime::ExecutionRole, turn::FollowUp, turn_machine::SessionEvent,
    },
};
use analysis::contexts::context::Context;
use commands::command::Command;
use common_models::{
    interaction::{Answer, PlanUpdate, Planning, Question, QuestionPurpose, WorkMode},
    tui_models::ActorToTuiPacket,
};
use utils::execution::ExecutionScope;

enum AnswerAction {
    Clarification,
    Merge(MergeDecision),
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub fn refresh_interaction(&self) {
        if matches!(self.workspace.runtime().role, ExecutionRole::Root) {
            self.workspace
                .runtime()
                .interaction
                .set(self.interaction.planning(), self.interaction.questions());
            self.reporter.send(ActorToTuiPacket::InteractionUpdated(
                self.interaction.view(),
            ));
        }
    }

    pub async fn sync_question_gate(&mut self) {
        self.refresh_interaction();
        self.dispatch(SessionEvent::QuestionsChanged(
            self.interaction.questions().gate(),
        ))
        .await;
    }

    pub(super) fn commit_interaction(&mut self, event: Event) -> anyhow::Result<()> {
        if matches!(self.persistence, Persistence::Ready) {
            self.persist(event);
        }
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }
    }

    fn apply_interaction(&mut self, update: InteractionUpdate) -> anyhow::Result<()> {
        self.interaction = update.commit(|event| self.commit_interaction(event))?;
        self.refresh_interaction();
        Ok(())
    }

    fn save_planning(&mut self, planning: Planning) -> anyhow::Result<()> {
        self.apply_interaction(self.interaction.planned(planning))
    }

    pub fn request_question(
        &mut self,
        question: Question,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        let update = Interaction::new(
            &self.interaction,
            &self.workspace.runtime().role,
            &self.persistence,
            scope,
        )?
        .ask(question)?;
        self.apply_interaction(update)?;
        self.interaction.content()
    }

    pub fn update_plan(
        &mut self,
        update: PlanUpdate,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        let update = Interaction::new(
            &self.interaction,
            &self.workspace.runtime().role,
            &self.persistence,
            scope,
        )?
        .update_plan(update)?;
        self.apply_interaction(update)?;
        let planning = self.interaction.planning();
        Ok(format!(
            "Plan updated: revision={}, requirements_revision={}.",
            planning.plan.revision, planning.requirements_revision
        ))
    }

    pub fn reconcile_plan(&mut self) {
        let result = self
            .interaction
            .requirements_changed()
            .and_then(|update| match update {
                Some(update) => self.apply_interaction(update),
                None => Ok(()),
            });
        if let Err(error) = result {
            self.persistence_failed(error);
        }
    }

    pub fn record_plan_evidence(&mut self, result: &tools::tool_defs::ToolResult) {
        let update = match (&result.outcome, result.invocation.name.as_ref()) {
            (Err(_), _) | (_, "update_plan" | "request_user_input") => Ok(()),
            (Ok(_), _) => {
                let mut planning = self.interaction.planning().clone();
                planning.record_evidence(
                    format!("tool:{}", result.id.id),
                    result.invocation.display.clone(),
                );
                self.save_planning(planning)
            }
        };
        if let Err(error) = update {
            self.persistence_failed(error);
        }
    }

    pub async fn interaction_command(&mut self, command: Command) {
        let result = match &command {
            Command::Plan => self.set_work_mode(WorkMode::Plan),
            Command::Implement => self.set_work_mode(WorkMode::Implement),
            Command::Questions => Ok(match self.interaction.questions().pending().is_empty() {
                true => "No pending questions.".into(),
                false => self
                    .interaction
                    .questions()
                    .pending()
                    .iter()
                    .map(|question| question.display())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            }),
            Command::Answer(input) => {
                let result = self.answer_question(&input.id, input.answer.clone()).await;
                self.sync_question_gate().await;
                result
            }
            Command::Steer(_) if !self.turn.accepts_input() => Err(anyhow::anyhow!(
                "Actor is stopping; correction was not accepted."
            )),
            Command::Steer(text) => {
                let follow_up = FollowUp::new(Some(format!(
                    "Updated requirements for the current task: {text}"
                )));
                self.dispatch(SessionEvent::Steer(follow_up)).await;
                Ok("Correction accepted. Active work and queued follow-ups are cancelled; the corrected task continues after cleanup.".into())
            }
            _ => Err(anyhow::anyhow!("Unsupported interaction command")),
        };
        self.reporter.send(ActorToTuiPacket::CommandResult(
            command,
            result.unwrap_or_else(|error| format!("{error:#}")),
        ));
    }

    fn set_work_mode(&mut self, mode: WorkMode) -> anyhow::Result<String> {
        let planning = match self.turn.is_idle() {
            true => Ok(Planning {
                mode,
                ..self.interaction.planning().clone()
            }),
            false => Err(anyhow::anyhow!(
                "Interrupt the active turn before changing modes; cleanup must finish before the new policy applies"
            )),
        }?;
        self.save_planning(planning)?;
        let mode = match mode {
            WorkMode::Plan => {
                "Plan mode: read-only investigation. Use /implement to return to implementation."
            }
            WorkMode::Implement => "Implementation mode: project tools are enabled.",
        };
        let steps = self
            .interaction
            .planning()
            .plan
            .steps
            .iter()
            .map(|step| {
                format!(
                    "{} [{:?}] {} — {}",
                    step.id, step.state, step.description, step.acceptance
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!("{mode}\n{steps}"))
    }

    pub(super) fn ask_question(&mut self, question: Question) -> anyhow::Result<()> {
        self.apply_interaction(self.interaction.asked(question)?)
    }

    pub(super) fn withdraw_questions(&mut self, purpose: QuestionPurpose) -> anyhow::Result<()> {
        if let Some(update) = self.interaction.withdrawn(purpose) {
            self.apply_interaction(update)?;
        }
        Ok(())
    }

    async fn answer_question(&mut self, id: &str, answer: Answer) -> anyhow::Result<String> {
        let answered = self.interaction.answered(id, answer.clone())?;
        let action = match answered.answer.purpose {
            QuestionPurpose::Clarification => AnswerAction::Clarification,
            QuestionPurpose::Merge => AnswerAction::Merge(self.merge_decision(id, &answer)?),
        };
        let message = clients::llm::Message::new(answered.answer.to_string());
        self.apply_interaction(answered.update)?;
        match self.turn.batch() {
            Some(_) => self.conversation.defer(message),
            None => self.conversation.push(message),
        }
        match action {
            AnswerAction::Clarification => Ok(format!("Answered question {id}.")),
            AnswerAction::Merge(decision) => self.answer_merge(decision).await,
        }
    }
}
