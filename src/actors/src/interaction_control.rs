use crate::{
    actor_state::ActorState, runtime::ExecutionRole, session::Event, session_control::Persistence,
    turn::FollowUp, turn_machine::SessionEvent,
};
use analysis::contexts::context::Context;
use commands::command::Command;
use common_models::{
    interaction::{Answer, InteractionView, PlanUpdate, Planning, Question, WorkMode},
    tui_models::ActorToTuiPacket,
};
use utils::execution::ExecutionScope;

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) fn interaction_instructions(&self) -> String {
        let guidance = match self.dependency.runtime.role {
            ExecutionRole::Root => include_str!("workers/resources/interaction.md"),
            ExecutionRole::Worker { .. } | ExecutionRole::Helper => {
                "Inherit the parent's work mode. Report questions, blockers, plan progress and evidence to the parent; only the root can update the shared plan or ask the user."
            }
        };
        format!(
            "Work mode: {:?}. Plan mode permits read-only investigation; Cargo and all workspace mutations are denied. Only the user can change modes. Questions and answers cannot change workspace permissions.\n{guidance}",
            self.dependency.runtime.interaction.mode()
        )
    }
    pub(crate) fn refresh_interaction(&self) {
        if matches!(self.dependency.runtime.role, ExecutionRole::Root) {
            self.dependency
                .runtime
                .interaction
                .set(&self.planning, &self.questions);
            self.reporter
                .send(ActorToTuiPacket::InteractionUpdated(InteractionView {
                    planning: self.planning.clone(),
                    questions: self.questions.pending().to_vec(),
                }));
        }
    }

    pub(crate) async fn sync_question_gate(&mut self) {
        self.refresh_interaction();
        self.dispatch(SessionEvent::QuestionsChanged(self.questions.gate()))
            .await;
    }

    fn interaction_content(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&serde_json::json!({
            "planning": self.planning,
            "pending_questions": self.questions.pending(),
        }))?)
    }

    fn commit_interaction(&mut self, event: Event) -> anyhow::Result<()> {
        if matches!(self.persistence, Persistence::Ready) {
            self.persist(event);
        }
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }
    }

    fn save_planning(&mut self, planning: Planning) -> anyhow::Result<()> {
        self.commit_interaction(Event::Planning(planning.clone()))?;
        self.planning = planning;
        self.refresh_interaction();
        Ok(())
    }

    pub(crate) fn reconcile_plan(&mut self) {
        if !self.planning.plan.steps.is_empty()
            && let Err(error) = self
                .planning
                .requirements_changed()
                .and_then(|planning| self.save_planning(planning))
        {
            self.persistence_failed(error);
        }
    }

    pub(crate) fn record_plan_evidence(&mut self, result: &tools::tool_defs::ToolResult) {
        if result.outcome.is_ok()
            && !matches!(
                result.invocation.name.as_ref(),
                "update_plan" | "request_user_input"
            )
        {
            self.planning.record_evidence(
                format!("tool:{}", result.id.id),
                result.invocation.display.clone(),
            );
            self.persist(Event::Planning(self.planning.clone()));
            self.refresh_interaction();
        }
    }

    pub(crate) async fn interaction_command(&mut self, command: Command) {
        let result = match &command {
            Command::Plan => self.set_work_mode(WorkMode::Plan),
            Command::Implement => self.set_work_mode(WorkMode::Implement),
            Command::Questions => Ok(match self.questions.pending().is_empty() {
                true => "No pending questions.".into(),
                false => self
                    .questions
                    .pending()
                    .iter()
                    .map(Question::display)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            }),
            Command::Answer(input) => {
                let result = self.answer_question(&input.id, input.answer.clone());
                if result.is_ok() {
                    self.sync_question_gate().await;
                }
                result
            }
            Command::Steer(text) => {
                let follow_up = FollowUp::new(Some(format!(
                    "Updated requirements for the current task: {text}"
                )));
                self.queue_input(&follow_up);
                self.dispatch(SessionEvent::Steer(follow_up)).await;
                Ok("Correction accepted. Active work and queued follow-ups are cancelled; the corrected task continues after cleanup.".into())
            }
            _ => Err(anyhow::anyhow!("Unsupported interaction command")),
        };
        self.reporter.send(ActorToTuiPacket::CommandResult(
            command,
            result.unwrap_or_else(|error| error.to_string()),
        ));
    }

    fn set_work_mode(&mut self, mode: WorkMode) -> anyhow::Result<String> {
        let planning = match self.turn.is_idle() {
            true => Ok(Planning {
                mode,
                ..self.planning.clone()
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
            .planning
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

    fn answer_question(&mut self, id: &str, answer: Answer) -> anyhow::Result<String> {
        let mut questions = self.questions.clone();
        let answered = questions.answer(id, &answer)?;
        let planning = self.planning.with_answer(&answered)?;
        self.commit_interaction(Event::QuestionAnswered {
            id: id.into(),
            answer,
        })?;
        self.questions = questions;
        self.planning = planning;
        let message = clients::llm::Message::new(answered.to_string());
        match self.turn.batch() {
            Some(_) => self.deferred_input.push(message),
            None => self.history.push(message),
        }
        self.refresh_interaction();
        Ok(format!("Answered question {id}."))
    }
}

pub(crate) struct Interaction<'a, C: Context> {
    state: &'a mut ActorState<C>,
}

impl<'a, C: Context + Clone + 'static> Interaction<'a, C> {
    pub fn new(state: &'a mut ActorState<C>, scope: &ExecutionScope) -> anyhow::Result<Self> {
        match (
            &state.persistence,
            scope.cancel.is_cancelled(),
            &state.dependency.runtime.role,
        ) {
            (Persistence::Ready, false, ExecutionRole::Root) => Ok(Self { state }),
            (Persistence::Failed(failure), _, _) => Err(anyhow::anyhow!(failure.to_string())),
            (_, true, _) => Err(anyhow::anyhow!("Interaction cancelled")),
            (_, _, ExecutionRole::Worker { .. } | ExecutionRole::Helper) => Err(anyhow::anyhow!(
                "Only the root can change the plan or ask the user; report questions to the parent"
            )),
        }
    }

    pub fn ask(self, question: Question) -> anyhow::Result<String> {
        let mut questions = self.state.questions.clone();
        questions.ask(question.clone())?;
        self.state
            .commit_interaction(Event::QuestionAsked(question))?;
        self.state.questions = questions;
        self.state.refresh_interaction();
        self.state.interaction_content()
    }

    pub fn update_plan(self, update: PlanUpdate) -> anyhow::Result<String> {
        let plan = self.state.planning.plan.update(
            update,
            self.state.planning.requirements_revision,
            &self.state.planning.evidence,
        )?;
        self.state.save_planning(Planning {
            plan,
            ..self.state.planning.clone()
        })?;
        self.state.interaction_content()
    }
}
