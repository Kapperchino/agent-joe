use super::runtime::Runtime;
use analysis::contexts::context::Context;
use clients::llm::Message;
use utils::execution::ExecutionScope;

#[derive(Clone)]
pub struct ActiveWorkspace<C: Context> {
    context: C,
    runtime: Runtime,
}

impl<C: Context> ActiveWorkspace<C> {
    pub fn new(mut context: C, runtime: Runtime) -> anyhow::Result<Self> {
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
        Ok(Self { context, runtime })
    }

    pub fn context(&self) -> &C {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut C {
        &mut self.context
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    pub fn worker_owner(&self) -> String {
        self.runtime
            .session
            .as_ref()
            .map(|session| session.id.clone())
            .unwrap_or_else(|| format!("actor-{}", self.context.get_id()))
    }

    pub async fn initial_history(&self) -> Vec<Message> {
        initial_history(&self.context).await
    }
}

pub async fn initial_history<C: Context>(context: &C) -> Vec<Message> {
    std::iter::once(Message::new(context.get_ctx().await))
        .chain(
            context
                .initial_task()
                .map(|task| Message::new(task.to_owned())),
        )
        .collect()
}

impl<C: Context + Clone> ActiveWorkspace<C> {
    pub fn relocated(&self, runtime: Runtime) -> anyhow::Result<Self> {
        let mut context = self.context.clone();
        context.clear_task_context();
        context.relocate(runtime.scope.workspace()?.root().to_path_buf())?;
        Ok(Self { context, runtime })
    }

    pub fn execution(
        &self,
        scope: ExecutionScope,
        constraints: impl IntoIterator<Item = String>,
    ) -> Self {
        let runtime = self.runtime.child(scope.clone());
        Self {
            context: self.context.clone(),
            runtime: Runtime {
                turn_scope: Some(scope),
                inherited_constraints: runtime
                    .inherited_constraints
                    .into_iter()
                    .chain(constraints)
                    .collect(),
                ..runtime
            },
        }
    }
}
