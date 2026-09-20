use super::{Event, PendingBatch, Session};
use clients::failure::{Failure, FailureKind};
use common_models::tui_models::{ActorToTuiPacket, EventSink};
use interaction::{InteractionEvent, control::InteractionPersistence};

pub struct SessionPersistence<'a> {
    pub state: &'a mut Persistence,
    pub session: Option<&'a Session>,
    pub reporter: &'a dyn EventSink,
}

impl SessionPersistence<'_> {
    pub fn prepare_batch(&mut self, batch: Option<&turn_engine::turn::ToolBatch>) {
        if let (Some(session), Some(batch)) = (self.session, batch) {
            self.record(Event::Prepared(PendingBatch::new(session, batch)));
        }
    }

    pub fn persist_report(&mut self, packet: &ActorToTuiPacket) {
        if let (
            Some(session),
            ActorToTuiPacket::TurnChanged {
                turn_id,
                state,
                detail,
            },
        ) = (self.session, packet)
        {
            self.record(Event::Status {
                turn: session.key(turn_id),
                state: *state,
                detail: detail.clone(),
            });
        }
    }

    pub fn record(&mut self, event: Event) {
        let result = self
            .session
            .map(|session| session.record(event))
            .transpose();
        if let Err(error) = result {
            self.fail(error);
        }
    }

    pub fn commit(&mut self, event: Event) -> anyhow::Result<()> {
        if matches!(self.state, Persistence::Ready) {
            self.record(event);
        }
        self.state.committed(()).map_err(Into::into)
    }

    pub fn fail(&mut self, error: anyhow::Error) {
        if let Some(message) = self.state.fail(error) {
            self.reporter.send(ActorToTuiPacket::SessionError(message));
        }
    }
}

pub enum Persistence {
    Ready,
    Failed(Failure),
}

impl Persistence {
    pub fn committed<T>(&self, value: T) -> Result<T, Failure> {
        match self {
            Self::Ready => Ok(value),
            Self::Failed(failure) => Err(failure.clone()),
        }
    }

    pub fn fail(&mut self, error: anyhow::Error) -> Option<String> {
        match self {
            Self::Ready => {
                let message =
                    format!("Session storage failed: {error}. Automatic continuation stopped.");
                *self = Self::Failed(Failure::new(FailureKind::Tool, message.clone()));
                Some(message)
            }
            Self::Failed(_) => None,
        }
    }
}

impl From<InteractionEvent> for Event {
    fn from(event: InteractionEvent) -> Self {
        match event {
            InteractionEvent::Planning(planning) => Self::Planning(planning),
            InteractionEvent::QuestionAsked(question) => Self::QuestionAsked(question),
            InteractionEvent::QuestionsWithdrawn(purpose) => Self::QuestionsWithdrawn(purpose),
            InteractionEvent::QuestionAnswered { id, answer } => {
                Self::QuestionAnswered { id, answer }
            }
        }
    }
}

impl InteractionPersistence for SessionPersistence<'_> {
    fn ready(&self) -> anyhow::Result<()> {
        self.state.committed(()).map_err(Into::into)
    }

    fn commit(&mut self, event: InteractionEvent) -> anyhow::Result<()> {
        SessionPersistence::commit(self, event.into())
    }

    fn fail(&mut self, error: anyhow::Error) {
        SessionPersistence::fail(self, error);
    }

    fn report(&self, packet: ActorToTuiPacket) {
        self.reporter.send(packet);
    }
}

impl merge_workflow::execution::MergePersistence for SessionPersistence<'_> {
    fn worktree(&self) -> anyhow::Result<merge_workflow::execution::MergeWorktree> {
        use merge_workflow::execution::MergeWorktree;
        match self.session {
            Some(session) => Ok(match session.snapshot()?.worktree {
                Some(worktree) => MergeWorktree::Isolated(worktree),
                None => MergeWorktree::Shared,
            }),
            None => Ok(MergeWorktree::Inactive),
        }
    }

    fn record_approval(&mut self, approval: merge_workflow::MergeApproval) -> anyhow::Result<()> {
        self.commit(Event::MergeApproval(approval))
    }

    fn clear_worktree(&mut self) {
        self.record(Event::Worktree(None));
    }
}
