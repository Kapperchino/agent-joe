use super::{
    Session, conversation::Conversation, interaction_state::InteractionState,
    session_merge::MergeApproval, session_transition::SessionTransition,
};
use crate::{
    states::{
        runtime::Runtime,
        workspace::{ActiveWorkspace, initial_history},
    },
    worker_registry::{WorkerRegistry, report::WorkerView},
};
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::tui_models::TokenCount;
use std::{collections::BTreeMap, sync::Arc};
use utils::git::worktrees::session::SessionWorktree;

pub(crate) struct SessionActivation<C: Context> {
    pub workspace: ActiveWorkspace<C>,
    pub conversation: Conversation,
    pub interaction: InteractionState,
    pub merge_approval: MergeApproval,
    pub usage: TokenCount,
    pub workers: WorkerRecovery,
}

pub(crate) enum WorkerRecovery {
    Fresh,
    Saved {
        workers: BTreeMap<String, WorkerView>,
    },
}

impl WorkerRecovery {
    pub fn apply(self, registry: &WorkerRegistry, owner: &str) {
        match self {
            Self::Fresh => {}
            Self::Saved { workers } => registry.restore(owner, workers),
        }
    }
}

impl<C: Context + Clone> SessionActivation<C> {
    pub async fn start(context: C, runtime: Runtime, client: &LLmClient) -> anyhow::Result<Self> {
        Self::fresh(context, runtime, client, SessionTransition::Start).await
    }

    pub async fn clear(workspace: &ActiveWorkspace<C>, client: &LLmClient) -> anyhow::Result<Self> {
        let mut context = workspace.context().clone();
        context.clear_task_context();
        Self::fresh(
            context,
            workspace.runtime().clone(),
            client,
            SessionTransition::Clear,
        )
        .await
    }

    async fn fresh(
        context: C,
        runtime: Runtime,
        client: &LLmClient,
        transition: SessionTransition,
    ) -> anyhow::Result<Self> {
        let interaction = match transition {
            SessionTransition::Start => InteractionState::new(runtime.interaction.mode()),
            SessionTransition::Clear => InteractionState::default(),
        };
        let history = initial_history(&context).await;
        let runtime = transition.apply(runtime, client, &history)?;
        let workspace = ActiveWorkspace::new(context, runtime)?;
        let history = workspace.initial_history().await;
        Ok(Self {
            conversation: Conversation::new(history, workspace.runtime().session.as_deref()),
            workspace,
            interaction,
            merge_approval: Default::default(),
            usage: Default::default(),
            workers: WorkerRecovery::Fresh,
        })
    }

    pub async fn resume(
        workspace: &ActiveWorkspace<C>,
        session: Arc<Session>,
        source: Option<&SessionWorktree>,
    ) -> anyhow::Result<Self> {
        let mut runtime = workspace.runtime().clone();
        runtime.session = Some(session.clone());
        runtime.activate_session(source)?;
        let snapshot = session.snapshot()?;
        runtime.scope.changes = session.change_tracker(snapshot.changes.clone());
        let mut context = workspace.context().clone();
        context.clear_task_context();
        let workspace = ActiveWorkspace::new(context, runtime)?;
        let fresh = Message::new(workspace.context().get_ctx().await);
        Ok(Self {
            conversation: Conversation::restored(&snapshot, fresh),
            interaction: InteractionState::restored(snapshot.planning, snapshot.questions),
            workspace,
            merge_approval: snapshot.merge_approval,
            usage: snapshot.usage,
            workers: WorkerRecovery::Saved {
                workers: snapshot.workers,
            },
        })
    }
}
