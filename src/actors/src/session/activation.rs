use super::{
    Session,
    conversation::{Conversation, SavedConversation},
    interaction_state::InteractionState,
    session_merge::MergeApproval,
    session_transition::SessionTransition,
};
use crate::{
    states::runtime::Runtime,
    worker_registry::{WorkerRegistry, report::WorkerView},
};
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::tui_models::TokenCount;
use std::{collections::BTreeMap, sync::Arc};
use utils::git::worktrees::session::SessionWorktree;

pub struct SessionActivation<C: Context> {
    pub context: C,
    pub runtime: Runtime,
    pub conversation: Conversation,
    pub interaction: InteractionState,
    pub merge_approval: MergeApproval,
    pub usage: TokenCount,
    pub workers: WorkerRecovery,
}

pub enum WorkerRecovery {
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

    pub async fn clear(context: &C, runtime: &Runtime, client: &LLmClient) -> anyhow::Result<Self> {
        let mut context = context.clone();
        context.clear_task_context();
        Self::fresh(context, runtime.clone(), client, SessionTransition::Clear).await
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
        let context = Self::relocate(context, &runtime)?;
        let history = initial_history(&context).await;
        Ok(Self {
            conversation: Conversation::new(
                history,
                runtime.session.as_ref().map(|session| session.id.clone()),
            ),
            context,
            runtime,
            interaction,
            merge_approval: Default::default(),
            usage: Default::default(),
            workers: WorkerRecovery::Fresh,
        })
    }

    pub async fn resume(
        context: &C,
        runtime: &Runtime,
        session: Arc<Session>,
        source: Option<&SessionWorktree>,
    ) -> anyhow::Result<Self> {
        let mut runtime = runtime.clone();
        runtime.session = Some(session.clone());
        runtime.activate_session(source)?;
        let snapshot = session.snapshot()?;
        runtime.scope.changes = session.change_tracker(snapshot.changes.clone());
        let mut context = context.clone();
        context.clear_task_context();
        let context = Self::relocate(context, &runtime)?;
        let fresh = Message::new(context.get_ctx().await);
        Ok(Self {
            conversation: Conversation::restored(
                SavedConversation {
                    cache_key: snapshot.id,
                    history: snapshot.history,
                    deferred_input: snapshot.deferred_input,
                    checkpoint: snapshot.context,
                },
                fresh,
            ),
            interaction: InteractionState::restored(snapshot.planning, snapshot.questions),
            context,
            runtime,
            merge_approval: snapshot.merge_approval,
            usage: snapshot.usage,
            workers: WorkerRecovery::Saved {
                workers: snapshot.workers,
            },
        })
    }
    fn relocate(mut context: C, runtime: &Runtime) -> anyhow::Result<C> {
        match runtime
            .session
            .as_ref()
            .map(|session| session.snapshot())
            .transpose()?
        {
            Some(snapshot) if snapshot.worktree.is_some() => {
                context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
            }
            _ => {}
        }
        Ok(context)
    }
}

async fn initial_history<C: Context>(context: &C) -> Vec<Message> {
    std::iter::once(Message::new(context.get_ctx().await))
        .chain(
            context
                .initial_task()
                .map(|task| Message::new(task.to_owned())),
        )
        .collect()
}
