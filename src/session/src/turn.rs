use crate::changes::SessionChanges;
use crate::control::SessionControl;
use crate::persistence::SessionPersistence;
use crate::state::{SessionAccess, SessionState};
use clients::llm::Message;
use clients::response::RequestMode;
use commands::command::{Command, QuestionAnswer};
use common_models::interaction::QuestionPurpose;
use common_models::runtime_ids::TurnId;
use interaction::access::InteractionReadiness;
use interaction::control::{InteractionAction, InteractionControl};
use merge_workflow::MergeEvent;
use merge_workflow::execution::{MergeActivity, MergeCompletion, MergeEnvironment, SessionMerge};
use tools::tool_error::{FailureImpact, ToolFailure, ToolFailureKind};
use turn_engine::machine::{Event, SessionEvent, TurnMachine};
use turn_engine::turn::{FollowUp, ToolJob};

pub struct SessionTurn<'a> {
    pub state: &'a mut SessionState,
    pub access: SessionAccess<'a>,
    pub environment: MergeEnvironment<'a>,
    pub turn: &'a TurnMachine,
}

impl SessionTurn<'_> {
    pub fn history(&mut self) -> SessionControl<'_> {
        self.state.control(self.access)
    }

    pub fn interaction(&mut self) -> InteractionControl<'_, SessionPersistence<'_>> {
        self.state.interaction_control(self.access)
    }

    pub fn merge(&mut self) -> SessionMerge<'_, SessionPersistence<'_>> {
        self.state.merge(
            self.access,
            self.environment,
            match self.turn.is_idle() {
                true => MergeActivity::Idle,
                false => MergeActivity::Active,
            },
        )
    }

    pub fn observe(&mut self, event: &Event) {
        if matches!(
            event,
            Event::StopRequested
                | Event::Shutdown
                | Event::Session(SessionEvent::Interrupt(_) | SessionEvent::Steer(_))
        ) {
            self.merge().pause_merge();
        }
    }

    pub async fn begin(&mut self, input: FollowUp, mode: RequestMode) {
        if let Err(error) = self
            .merge()
            .record_merge(MergeEvent::TaskStarted { turn: input.id })
        {
            self.history().persistence.fail(error);
        }
        self.interaction().refresh_interaction();
        if let (Some(_), None) = (
            &input.prompt,
            self.state.merge_approval.resolution(input.id),
        ) {
            self.interaction().reconcile_plan();
        }
        if let Err(error) = SessionChanges::begin_turn(self.environment.scope, mode).await {
            self.history().persistence.fail(error);
        }
        self.history().begin_turn(input);
    }

    pub fn command(&mut self, command: &Command) -> anyhow::Result<InteractionAction> {
        let readiness = match (self.turn.is_idle(), self.turn.accepts_input()) {
            (true, _) => InteractionReadiness::Idle,
            (false, true) => InteractionReadiness::Active,
            (false, false) => InteractionReadiness::Stopping,
        };
        self.interaction().command(command, readiness)
    }

    pub fn compact(&mut self) -> anyhow::Result<SessionEvent> {
        match self.turn.is_idle() {
            true => {
                let input = FollowUp::new(None);
                self.state.conversation.compact(input.id);
                Ok(SessionEvent::Start(input))
            }
            false => Err(anyhow::anyhow!(
                "Interrupt the active turn before compacting manually. Automatic compaction runs between complete tool exchanges."
            )),
        }
    }

    pub async fn answer(
        &mut self,
        input: &QuestionAnswer,
    ) -> anyhow::Result<Option<MergeCompletion>> {
        let answered = self
            .state
            .interaction
            .answered(&input.id, input.answer.clone())?;
        let decision = match answered.answer.purpose {
            QuestionPurpose::Clarification => None,
            QuestionPurpose::Merge => Some(self.merge().merge_decision(&input.id, &input.answer)?),
        };
        let message = Message::new(answered.answer.to_string());
        self.interaction().apply_interaction(answered.update)?;
        match self.turn.batch() {
            Some(_) => self.state.conversation.defer(message),
            None => self.state.conversation.push(message),
        }
        match decision {
            Some(decision) => self.merge().answer_merge(decision).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn offer_merge(
        &mut self,
        turn: TurnId,
        mode: RequestMode,
    ) -> anyhow::Result<Option<MergeCompletion>> {
        let mode = self.state.conversation.request_mode(turn, mode);
        self.merge().offer_merge(turn, mode).await
    }

    pub fn prepare_tools(&mut self, jobs: Vec<ToolJob>) -> Result<Vec<ToolJob>, ToolFailure> {
        let batch = self.turn.batch();
        self.history().persistence.prepare_batch(batch);
        self.state.persistence.committed(jobs).map_err(|failure| {
            ToolFailure::new(
                ToolFailureKind::Persistence,
                FailureImpact::NotStarted,
                failure.to_string(),
            )
        })
    }
}
