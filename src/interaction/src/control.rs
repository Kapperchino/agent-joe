use crate::access::{Interaction, InteractionReadiness, InteractionRole};
use crate::policy::InteractionPolicy;
use crate::{InteractionState, InteractionUpdate};
use commands::command::Command;
use common_models::{
    interaction::{PlanUpdate, Planning, Question, QuestionPurpose, WorkMode},
    tui_models::ActorToTuiPacket,
};
use turn_engine::turn::FollowUp;
use utils::execution::ExecutionScope;

pub enum InteractionAction {
    Reply(String),
    Answer(commands::command::QuestionAnswer),
    Steer(FollowUp),
}

pub trait InteractionPersistence {
    fn ready(&self) -> anyhow::Result<()>;
    fn commit(&mut self, event: crate::InteractionEvent) -> anyhow::Result<()>;
    fn fail(&mut self, error: anyhow::Error);
    fn report(&self, packet: ActorToTuiPacket);
}

pub struct InteractionControl<'a, P: InteractionPersistence> {
    pub state: &'a mut InteractionState,
    pub persistence: P,
    pub policy: &'a InteractionPolicy,
    pub role: InteractionRole,
}

impl<P: InteractionPersistence> InteractionControl<'_, P> {
    pub fn refresh_interaction(&self) {
        if matches!(self.role, InteractionRole::Root) {
            self.policy
                .set(self.state.planning(), self.state.questions());
            self.persistence
                .report(ActorToTuiPacket::InteractionUpdated(self.state.view()));
        }
    }

    pub fn apply_interaction(&mut self, update: InteractionUpdate) -> anyhow::Result<()> {
        *self.state = update.commit(|event| self.persistence.commit(event))?;
        self.refresh_interaction();
        Ok(())
    }

    fn save_planning(&mut self, planning: Planning) -> anyhow::Result<()> {
        self.apply_interaction(self.state.planned(planning))
    }

    pub fn request_question(
        &mut self,
        question: Question,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.persistence.ready()?;
        let update = Interaction::new(self.state, self.role, scope)?.ask(question)?;
        self.apply_interaction(update)?;
        self.state.content()
    }

    pub fn update_plan(
        &mut self,
        update: PlanUpdate,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.persistence.ready()?;
        let update = Interaction::new(self.state, self.role, scope)?.update_plan(update)?;
        self.apply_interaction(update)?;
        let planning = self.state.planning();
        Ok(format!(
            "Plan updated: revision={}, requirements_revision={}.",
            planning.plan.revision, planning.requirements_revision
        ))
    }

    pub fn reconcile_plan(&mut self) {
        let result = self
            .state
            .requirements_changed()
            .and_then(|update| match update {
                Some(update) => self.apply_interaction(update),
                None => Ok(()),
            });
        if let Err(error) = result {
            self.persistence.fail(error);
        }
    }

    pub fn record_plan_evidence(&mut self, result: &tools::tool_defs::ToolResult) {
        let update = match (&result.outcome, result.invocation.name.as_ref()) {
            (Err(_), _) | (_, "update_plan" | "request_user_input") => Ok(()),
            (Ok(_), _) => {
                let mut planning = self.state.planning().clone();
                planning.record_evidence(
                    format!("tool:{}", result.id.id),
                    result.invocation.display.clone(),
                );
                self.save_planning(planning)
            }
        };
        if let Err(error) = update {
            self.persistence.fail(error);
        }
    }

    pub fn command(
        &mut self,
        command: &Command,
        readiness: InteractionReadiness,
    ) -> anyhow::Result<InteractionAction> {
        match command {
            Command::Plan => self
                .set_work_mode(WorkMode::Plan, readiness)
                .map(InteractionAction::Reply),
            Command::Implement => self
                .set_work_mode(WorkMode::Implement, readiness)
                .map(InteractionAction::Reply),
            Command::Questions => Ok(InteractionAction::Reply(
                match self.state.questions().pending() {
                    [] => "No pending questions.".into(),
                    questions => questions
                        .iter()
                        .map(|question| question.display())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                },
            )),
            Command::Answer(input) => Ok(InteractionAction::Answer(input.clone())),
            Command::Steer(_) if matches!(readiness, InteractionReadiness::Stopping) => Err(
                anyhow::anyhow!("Actor is stopping; correction was not accepted."),
            ),
            Command::Steer(text) => Ok(InteractionAction::Steer(FollowUp::new(Some(format!(
                "Updated requirements for the current task: {text}"
            ))))),
            _ => Err(anyhow::anyhow!("Unsupported interaction command")),
        }
    }

    fn set_work_mode(
        &mut self,
        mode: WorkMode,
        readiness: InteractionReadiness,
    ) -> anyhow::Result<String> {
        let planning = match readiness {
            InteractionReadiness::Idle => Ok(Planning {
                mode,
                ..self.state.planning().clone()
            }),
            InteractionReadiness::Active | InteractionReadiness::Stopping => Err(anyhow::anyhow!(
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
            .state
            .planning()
            .plan
            .steps
            .iter()
            .map(|step| {
                format!(
                    "{} [{:?}, {:?}] {} — {}",
                    step.id, step.kind, step.state, step.description, step.acceptance
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!("{mode}\n{steps}"))
    }

    pub fn ask_question(&mut self, question: Question) -> anyhow::Result<()> {
        self.apply_interaction(self.state.asked(question)?)
    }

    pub fn withdraw_questions(&mut self, purpose: QuestionPurpose) -> anyhow::Result<()> {
        if let Some(update) = self.state.withdrawn(purpose) {
            self.apply_interaction(update)?;
        }
        Ok(())
    }
}
