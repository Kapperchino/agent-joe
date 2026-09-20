use crate::states::runtime::{ExecutionRole, Runtime};
use clients::llm::{LLmClient, Message};

pub enum SessionTransition {
    Start,
    Clear,
}

impl SessionTransition {
    pub fn apply(
        self,
        mut runtime: Runtime,
        client: &LLmClient,
        history: &[Message],
    ) -> anyhow::Result<Runtime> {
        let parent = match self {
            Self::Start => runtime.session.as_ref().map(|session| session.id.clone()),
            Self::Clear => None,
        };
        runtime.session = match &runtime.sessions {
            Some(store) => {
                Some(store.create(client.session_provider(), parent, history.to_vec())?)
            }
            None => runtime.session,
        };
        runtime.activate_session(None)?;
        runtime.scope.changes = match self {
            Self::Start => {
                if let ExecutionRole::Worker { session, .. } = &runtime.role {
                    session.attach(runtime.session.clone())?;
                }
                match &runtime.session {
                    Some(session) if session.snapshot()?.parent.is_none() => {
                        session.change_tracker(Default::default())
                    }
                    _ => runtime.scope.changes,
                }
            }
            Self::Clear => runtime
                .session
                .as_ref()
                .map(|session| session.change_tracker(Default::default()))
                .unwrap_or_default(),
        };
        Ok(runtime)
    }
}
