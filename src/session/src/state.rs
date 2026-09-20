use crate::Session;
use crate::control::SessionControl;
use crate::persistence::{Persistence, SessionPersistence};
use common_models::tui_models::EventSink;
use conversation::Conversation;
use interaction::InteractionState;
use interaction::access::InteractionRole;
use interaction::control::InteractionControl;
use interaction::policy::InteractionPolicy;
use merge_workflow::MergeApproval;
use merge_workflow::execution::{MergeActivity, MergeEnvironment, SessionMerge};

pub struct SessionState {
    pub conversation: Conversation,
    pub interaction: InteractionState,
    pub persistence: Persistence,
    pub merge_approval: MergeApproval,
}

#[derive(Clone, Copy)]
pub struct SessionAccess<'a> {
    pub session: Option<&'a Session>,
    pub reporter: &'a dyn EventSink,
    pub policy: &'a InteractionPolicy,
    pub role: InteractionRole,
}

impl<'a> SessionAccess<'a> {
    fn persistence(self, state: &'a mut Persistence) -> SessionPersistence<'a> {
        SessionPersistence {
            state,
            session: self.session,
            reporter: self.reporter,
        }
    }
}

impl SessionState {
    pub fn worker_result(
        &self,
        outcome: turn_engine::machine::WorkerOutcome,
    ) -> Result<String, turn_engine::WorkerFailure> {
        self.persistence
            .committed(())
            .map_err(turn_engine::WorkerFailure::Turn)?;
        match outcome {
            turn_engine::machine::WorkerOutcome::Completed => Ok(self
                .conversation
                .history()
                .last()
                .map(clients::llm::Message::text)
                .unwrap_or_default()),
            turn_engine::machine::WorkerOutcome::Failed(failure) => Err(failure),
        }
    }

    pub fn new(
        conversation: Conversation,
        interaction: InteractionState,
        merge_approval: MergeApproval,
    ) -> Self {
        Self {
            conversation,
            interaction,
            persistence: Persistence::Ready,
            merge_approval,
        }
    }

    pub fn control<'a>(&'a mut self, access: SessionAccess<'a>) -> SessionControl<'a> {
        SessionControl {
            conversation: &mut self.conversation,
            persistence: access.persistence(&mut self.persistence),
        }
    }

    pub fn interaction_control<'a>(
        &'a mut self,
        access: SessionAccess<'a>,
    ) -> InteractionControl<'a, SessionPersistence<'a>> {
        InteractionControl {
            state: &mut self.interaction,
            persistence: access.persistence(&mut self.persistence),
            policy: access.policy,
            role: access.role,
        }
    }

    pub fn merge<'a>(
        &'a mut self,
        access: SessionAccess<'a>,
        environment: MergeEnvironment<'a>,
        activity: MergeActivity,
    ) -> SessionMerge<'a, SessionPersistence<'a>> {
        SessionMerge {
            approval: &mut self.merge_approval,
            interaction: InteractionControl {
                state: &mut self.interaction,
                persistence: access.persistence(&mut self.persistence),
                policy: access.policy,
                role: access.role,
            },
            environment,
            activity,
        }
    }
}
