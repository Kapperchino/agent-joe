use super::Session;
use super::transition::SessionTransition;
use crate::runtime::SessionRuntime;
use crate::state::SessionState;
use analysis::contexts::context::Context;
use clients::llm::{LLmClient, Message};
use common_models::tui_models::TokenCount;
use conversation::{Conversation, SavedConversation};
use interaction::InteractionState;
use std::{collections::BTreeMap, sync::Arc};
use utils::git::worktrees::session::SessionWorktree;
use worker_registry::WorkerRegistry;
use worker_registry::report::WorkerView;

pub struct SessionActivation<C: Context> {
    pub context: C,
    pub runtime: SessionRuntime,
    pub state: SessionState,
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
    pub async fn start(
        context: C,
        runtime: SessionRuntime,
        client: &LLmClient,
    ) -> anyhow::Result<Self> {
        Self::fresh(context, runtime, client, SessionTransition::Start).await
    }

    pub async fn clear(
        context: &C,
        runtime: &SessionRuntime,
        client: &LLmClient,
    ) -> anyhow::Result<Self> {
        let mut context = context.clone();
        context.clear_task_context();
        Self::fresh(context, runtime.clone(), client, SessionTransition::Clear).await
    }

    pub async fn from_plan(
        context: &C,
        runtime: &SessionRuntime,
        client: &LLmClient,
        handoff: &crate::plan::PlanHandoff,
    ) -> anyhow::Result<Self> {
        let mut context = context.clone();
        context.clear_task_context();
        let mut activation = Self::fresh(
            context,
            runtime.clone(),
            client,
            SessionTransition::PlanHandoff,
        )
        .await?;
        let message = Message::new(handoff.prompt.clone());
        if let Some(session) = &activation.runtime.session {
            session.record(crate::Event::Planning(handoff.planning.clone()))?;
            session.record(crate::Event::History(vec![message.clone()]))?;
        }
        activation.state.interaction =
            InteractionState::restored(handoff.planning.clone(), Default::default());
        activation.state.conversation.push(message);
        Ok(activation)
    }

    async fn fresh(
        context: C,
        runtime: SessionRuntime,
        client: &LLmClient,
        transition: SessionTransition,
    ) -> anyhow::Result<Self> {
        let interaction = match transition {
            SessionTransition::Start => InteractionState::new(runtime.interaction.mode()),
            SessionTransition::Clear | SessionTransition::PlanHandoff => {
                InteractionState::default()
            }
        };
        let history = initial_history(&context).await;
        let previous = runtime.scope.workspace().ok();
        let runtime = transition.apply(runtime, client, &history)?;
        let context = Self::relocate(context, &runtime, previous.as_deref())?;
        let history = initial_history(&context).await;
        Ok(Self {
            state: SessionState::new(
                Conversation::new(
                    history,
                    runtime.session.as_ref().map(|session| session.id.clone()),
                ),
                interaction,
                Default::default(),
            ),
            context,
            runtime,
            usage: Default::default(),
            workers: WorkerRecovery::Fresh,
        })
    }

    pub async fn resume(
        context: &C,
        runtime: &SessionRuntime,
        session: Arc<Session>,
        source: Option<&SessionWorktree>,
    ) -> anyhow::Result<Self> {
        let previous = runtime.scope.workspace().ok();
        let mut runtime = runtime.clone();
        runtime.session = Some(session.clone());
        let snapshot = session.snapshot()?;
        runtime.scope.changes = session.change_tracker(snapshot.changes.clone());
        runtime.activate_session(source)?;
        let mut context = context.clone();
        context.clear_task_context();
        let context = Self::relocate(context, &runtime, previous.as_deref())?;
        let fresh = Message::new(context.get_ctx().await);
        Ok(Self {
            state: SessionState::new(
                Conversation::restored(
                    SavedConversation {
                        cache_key: snapshot.id,
                        history: snapshot.history,
                        deferred_input: snapshot.deferred_input,
                        checkpoint: snapshot.context,
                    },
                    fresh,
                ),
                InteractionState::restored(snapshot.planning, snapshot.questions),
                snapshot.merge_approval,
            ),
            context,
            runtime,
            usage: snapshot.usage,
            workers: WorkerRecovery::Saved {
                workers: snapshot.workers,
            },
        })
    }
    fn relocate(
        mut context: C,
        runtime: &SessionRuntime,
        previous: Option<&utils::workspace::WorkspacePolicy>,
    ) -> anyhow::Result<C> {
        match (previous, runtime.scope.workspace()) {
            (Some(previous), Ok(workspace)) if previous.root() != workspace.root() => {
                context.relocate(workspace.root().to_path_buf())?;
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
