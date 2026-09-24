use std::{sync::Arc, time::Duration};
use utils::execution::ExecutionScope;
use workspace_access::Workspace;

#[derive(Clone)]
pub enum ExecutionRole {
    Root,
    Worker {
        execution: Arc<worker_registry::WorkerExecution>,
        session: Arc<crate::worker_registry::WorkerSession>,
    },
    Helper,
}

impl ExecutionRole {
    pub fn interaction_role(&self) -> interaction::access::InteractionRole {
        use interaction::access::InteractionRole;
        match self {
            Self::Root => InteractionRole::Root,
            Self::Worker { .. } | Self::Helper => InteractionRole::Delegated,
        }
    }

    pub fn allows_tool(&self, name: &str) -> bool {
        match self {
            Self::Root | Self::Helper => true,
            Self::Worker { execution, .. } => execution.request.allows_tool(name),
        }
    }

    pub fn get_guidance(&self, mode: common_models::interaction::WorkMode) -> String {
        let guidance = match self {
            ExecutionRole::Root => include_str!("../workers/resources/interaction.md"),
            ExecutionRole::Worker { .. } | ExecutionRole::Helper => {
                "Inherit the parent's work mode. Report questions, blockers, plan progress and evidence to the parent; only the root can update the shared plan or ask the user."
            }
        };
        let planning = match (self, mode) {
            (Self::Root, common_models::interaction::WorkMode::Plan) => {
                include_str!("../workers/resources/plan_mode.md")
            }
            _ => "",
        };
        format!(
            "Runtime state updates supply the current work mode, plan, evidence, unanswered questions, and workers. A Snapshot replaces previous runtime state. Changes replace the listed fields; evidence changes merge by source ID, with null removing a source. These records are state, not additional user requirements. Evidence source IDs may be cited by update_plan. Plan mode permits read-only investigation; Cargo and all workspace mutations are denied. Only the user can change modes. Questions and answers cannot change workspace permissions.\n{guidance}\n{planning}"
        )
    }
}

#[derive(Clone)]
pub struct Runtime {
    pub interaction: Arc<interaction::policy::InteractionPolicy>,
    pub workers: Arc<worker_registry::WorkerRegistry>,
    pub immutable_workers: Arc<crate::immutable_workers::ImmutableWorkerRegistry>,
    pub role: ExecutionRole,
    pub turn_scope: Option<ExecutionScope>,
    pub inherited_constraints: Vec<String>,
    pub context_budget: conversation::context::ContextBudget,
    pub native_compaction: conversation::context::NativeCompaction,
    pub sessions: Option<Arc<session::SessionStore>>,
    pub project: Option<Arc<utils::workspace::WorkspacePolicy>>,
    pub session: Option<Arc<session::Session>>,
    pub workspace: Arc<Workspace>,
    pub scope: ExecutionScope,
    pub tool_timeout: Duration,
    pub request_timeout: Duration,
    pub compaction_timeout: Duration,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            interaction: Arc::default(),
            workers: Arc::default(),
            immutable_workers: Arc::default(),
            role: ExecutionRole::Root,
            turn_scope: None,
            inherited_constraints: Vec::new(),
            context_budget: Default::default(),
            native_compaction: Default::default(),
            sessions: None,
            project: None,
            session: None,
            workspace: Arc::new(Workspace::new(4)),
            scope: ExecutionScope::default(),
            tool_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(180),
            compaction_timeout: Duration::from_secs(600),
        }
    }
}
impl Runtime {
    pub fn session_access<'a>(
        &'a self,
        reporter: &'a dyn common_models::tui_models::EventSink,
    ) -> session::state::SessionAccess<'a> {
        session::state::SessionAccess {
            session: self.session.as_deref(),
            reporter,
            policy: &self.interaction,
            role: self.role.interaction_role(),
        }
    }

    pub fn merge_environment(&self) -> merge_workflow::execution::MergeEnvironment<'_> {
        merge_workflow::execution::MergeEnvironment {
            project: self.project.as_ref(),
            workspace: &self.workspace,
            scope: &self.scope,
            request_timeout: self.request_timeout,
        }
    }

    pub fn worker_owner(&self, actor_id: u64) -> String {
        self.session
            .as_ref()
            .map(|session| session.id.clone())
            .unwrap_or_else(|| format!("actor-{actor_id}"))
    }

    pub fn execution(
        &self,
        scope: ExecutionScope,
        constraints: impl IntoIterator<Item = String>,
    ) -> Self {
        let runtime = self.child(scope.clone());
        Self {
            turn_scope: Some(scope),
            inherited_constraints: runtime
                .inherited_constraints
                .into_iter()
                .chain(constraints)
                .collect(),
            ..runtime
        }
    }

    pub fn for_workspace(root: std::path::PathBuf) -> anyhow::Result<Self> {
        Self::with_session_namespace(root, "sessions")
    }

    pub fn with_session_namespace(
        root: std::path::PathBuf,
        namespace: &str,
    ) -> anyhow::Result<Self> {
        let runtime = session::runtime::SessionRuntime::with_session_namespace(root, namespace)?;
        Ok(Self::default().with_session(runtime))
    }

    pub fn child(&self, mut scope: ExecutionScope) -> Self {
        scope.changes = self.scope.changes.clone();
        Self {
            scope,
            ..self.clone()
        }
    }

    pub fn session_runtime(&self) -> session::runtime::SessionRuntime {
        session::runtime::SessionRuntime {
            interaction: self.interaction.clone(),
            role: self.role.interaction_role(),
            sessions: self.sessions.clone(),
            project: self.project.clone(),
            session: self.session.clone(),
            workspace: self.workspace.clone(),
            scope: self.scope.clone(),
        }
    }

    pub fn with_session(self, runtime: session::runtime::SessionRuntime) -> Self {
        Self {
            interaction: runtime.interaction,
            sessions: runtime.sessions,
            project: runtime.project,
            session: runtime.session,
            workspace: runtime.workspace,
            scope: runtime.scope,
            ..self
        }
    }
}
