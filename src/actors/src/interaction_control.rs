use crate::{
    actor_state::ActorState, session::Event, session_control::Persistence, turn::FollowUp,
    turn_machine::SessionEvent,
};
use analysis::contexts::context::Context;
use commands::command::Command;
use common_models::{
    interaction::{Answer, InteractionView, PlanUpdate, Question, WorkMode},
    tui_models::ActorToTuiPacket,
};
use utils::execution::ExecutionScope;

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) fn interaction_instructions(&self) -> String {
        let guidance = match self.dependency.runtime.worker.is_none() {
            true => include_str!("workers/resources/interaction.md"),
            false => {
                "Inherit the parent's work mode. Report questions, blockers, plan progress and evidence to the parent; only the root can update the shared plan or ask the user."
            }
        };
        format!(
            "Work mode: {:?}. Plan mode permits read-only investigation; Cargo and all workspace mutations are denied. Only the user can change modes. Questions and answers cannot change workspace permissions.\n{guidance}",
            self.dependency.runtime.interaction.mode()
        )
    }
    fn interaction_ready(&self, scope: &ExecutionScope) -> anyhow::Result<()> {
        match (
            &self.persistence,
            scope.cancel.is_cancelled(),
            &self.dependency.runtime.worker,
        ) {
            (Persistence::Ready, false, None) => Ok(()),
            (Persistence::Failed(failure), _, _) => Err(anyhow::anyhow!(failure.to_string())),
            (_, true, _) => Err(anyhow::anyhow!("Interaction cancelled")),
            (_, _, Some(_)) => Err(anyhow::anyhow!(
                "Only the root can change the plan or ask the user; report questions to the parent"
            )),
        }
    }

    pub(crate) fn refresh_interaction(&self) {
        if self.dependency.runtime.worker.is_none() {
            self.dependency.runtime.interaction.set(
                self.planning.mode,
                self.questions.iter().any(|question| question.required),
                self.planning.plan.requirements_revision != self.planning.requirements_revision,
            );
            self.reporter
                .send(ActorToTuiPacket::InteractionUpdated(InteractionView {
                    planning: self.planning.clone(),
                    questions: self.questions.clone(),
                }));
        }
    }

    pub(crate) async fn sync_question_gate(&mut self) {
        self.refresh_interaction();
        self.dispatch(SessionEvent::QuestionsPending(
            self.questions.iter().any(|question| question.required),
        ))
        .await;
    }

    fn interaction_content(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&serde_json::json!({
            "planning": self.planning,
            "pending_questions": self.questions,
        }))?)
    }

    fn commit_interaction(&mut self, event: Event) -> anyhow::Result<()> {
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }?;
        self.persist(event);
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }
    }

    pub(crate) fn ask_question(
        &mut self,
        question: Question,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.interaction_ready(scope)?;
        match self.questions.len() < 8
            && self.answered_questions.len() + self.questions.len() < 256
            && !self
                .questions
                .iter()
                .any(|pending| pending.id == question.id)
            && !self.answered_questions.contains(&question.id)
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Question IDs must be unused; at most eight questions may be pending and 256 may be answered per session"
            )),
        }?;
        self.commit_interaction(Event::QuestionAsked(question.clone()))?;
        self.questions.push(question);
        self.refresh_interaction();
        self.interaction_content()
    }

    pub(crate) fn update_plan(
        &mut self,
        update: PlanUpdate,
        scope: &ExecutionScope,
    ) -> anyhow::Result<String> {
        self.interaction_ready(scope)?;
        let plan = self.planning.plan.update(
            update,
            self.planning.requirements_revision,
            &self.planning.evidence,
        )?;
        let planning = common_models::interaction::Planning {
            plan,
            ..self.planning.clone()
        };
        self.commit_interaction(Event::Planning(planning.clone()))?;
        self.planning = planning;
        self.refresh_interaction();
        self.interaction_content()
    }

    pub(crate) fn reconcile_plan(&mut self) {
        if !self.planning.plan.steps.is_empty() {
            let mut planning = self.planning.clone();
            match planning
                .reconcile()
                .and_then(|()| self.commit_interaction(Event::Planning(planning.clone())))
            {
                Ok(()) => {
                    self.planning = planning;
                    self.refresh_interaction();
                }
                Err(error) => self.persistence_failed(error),
            }
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
            Command::Questions => Ok(match self.questions.is_empty() {
                true => "No pending questions.".into(),
                false => self
                    .questions
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
        match self.turn.is_idle() {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Interrupt the active turn before changing modes; cleanup must finish before the new policy applies"
            )),
        }?;
        let planning = common_models::interaction::Planning {
            mode,
            ..self.planning.clone()
        };
        self.commit_interaction(Event::Planning(planning.clone()))?;
        self.planning = planning;
        self.refresh_interaction();
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
        let question = self
            .questions
            .iter()
            .find(|question| question.id == id)
            .ok_or_else(|| anyhow::anyhow!("Question {id} is not pending"))?;
        let text = question.answer(&answer)?;
        let message = clients::llm::Message::new(format!(
            "Answer to question {id} ({}): {text}",
            question.prompt
        ));
        self.commit_interaction(Event::QuestionAnswered {
            id: id.into(),
            answer,
        })?;
        self.questions.retain(|question| question.id != id);
        self.answered_questions.insert(id.into());
        match self.turn.batch().is_some() {
            true => self.deferred_input.push(message),
            false => self.history.push(message),
        }
        self.planning.record_evidence(format!("answer:{id}"), text);
        self.reconcile_plan();
        self.commit_interaction(Event::Planning(self.planning.clone()))?;
        self.refresh_interaction();
        Ok(format!("Answered question {id}."))
    }
}
