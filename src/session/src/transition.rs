use crate::runtime::SessionRuntime;
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use interaction::access::InteractionRole;

pub enum SessionTransition {
    Start,
    Clear,
}

pub struct SessionRelocation<C: Context> {
    pub context: C,
    pub runtime: SessionRuntime,
    pub message: Message,
}

impl<C: Context + Clone> SessionRelocation<C> {
    pub async fn new(context: &C, runtime: SessionRuntime) -> anyhow::Result<Self> {
        let mut context = context.clone();
        context.clear_task_context();
        context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        let message = Message::new(context.get_ctx().await);
        Ok(Self {
            context,
            runtime,
            message,
        })
    }

    pub async fn prepare(context: &C, runtime: &SessionRuntime) -> anyhow::Result<Option<Self>> {
        let mut runtime = runtime.clone();
        let session = match (&runtime.role, &runtime.project, &runtime.session) {
            (InteractionRole::Root, Some(_), Some(session))
                if session.snapshot()?.worktree.is_none() =>
            {
                Some(session.clone())
            }
            _ => None,
        };
        match session {
            Some(session) => {
                runtime.activate_session(None)?;
                let snapshot = session.snapshot()?;
                match snapshot.worktree {
                    Some(_) => {
                        runtime.scope.changes = session.change_tracker(snapshot.changes);
                        Self::new(context, runtime).await.map(Some)
                    }
                    None => Ok(None),
                }
            }
            None => Ok(None),
        }
    }
}

impl SessionTransition {
    pub fn apply(
        self,
        mut runtime: SessionRuntime,
        client: &LLmClient,
        history: &[Message],
    ) -> anyhow::Result<SessionRuntime> {
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
            Self::Start => match &runtime.session {
                Some(session) if session.snapshot()?.parent.is_none() => {
                    session.change_tracker(Default::default())
                }
                _ => runtime.scope.changes,
            },
            Self::Clear => runtime
                .session
                .as_ref()
                .map(|session| session.change_tracker(Default::default()))
                .unwrap_or_default(),
        };
        Ok(runtime)
    }
}
