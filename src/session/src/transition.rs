use crate::runtime::SessionRuntime;
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};

pub enum SessionTransition {
    Start,
    Clear,
    PlanHandoff,
}

pub struct SessionRelocation<C: Context> {
    pub context: C,
    pub runtime: SessionRuntime,
    pub message: Message,
}

impl<C: Context + Clone> SessionRelocation<C> {
    pub async fn new(context: &C, runtime: SessionRuntime) -> anyhow::Result<Self> {
        let mut context = context.clone();
        context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        let message = Message::new(context.get_ctx().await);
        Ok(Self {
            context,
            runtime,
            message,
        })
    }

    pub async fn prepare(context: &C, runtime: &SessionRuntime) -> anyhow::Result<Option<Self>> {
        let mut updated = runtime.clone();
        updated.prepare_workspace(tools::tool_defs::ToolOpKind::Read)?;
        match (runtime.scope.workspace(), updated.scope.workspace()) {
            (Ok(previous), Ok(workspace))
                if previous.root() != workspace.root()
                    || !std::sync::Arc::ptr_eq(&runtime.scope.changes, &updated.scope.changes) =>
            {
                Self::new(context, updated).await.map(Some)
            }
            _ => Ok(None),
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
            Self::Clear | Self::PlanHandoff => None,
        };
        let source = match self {
            Self::PlanHandoff => runtime
                .session
                .as_ref()
                .map(|session| session.snapshot())
                .transpose()?
                .and_then(|snapshot| snapshot.worktree.or(snapshot.worktree_source)),
            Self::Start | Self::Clear => None,
        };
        runtime.session = match &runtime.sessions {
            Some(store) => {
                Some(store.create(client.session_provider(), parent, history.to_vec())?)
            }
            None => runtime.session,
        };
        runtime.scope.changes = match self {
            Self::Start => match &runtime.session {
                Some(session) if session.snapshot()?.parent.is_none() => {
                    session.change_tracker(Default::default())
                }
                _ => runtime.scope.changes,
            },
            Self::Clear | Self::PlanHandoff => runtime
                .session
                .as_ref()
                .map(|session| session.change_tracker(Default::default()))
                .unwrap_or_default(),
        };
        runtime.activate_session(source.as_ref())?;
        Ok(runtime)
    }
}
