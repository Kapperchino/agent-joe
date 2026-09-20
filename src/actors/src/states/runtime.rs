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
    pub fn allows_tool(&self, name: &str) -> bool {
        match self {
            Self::Root | Self::Helper => true,
            Self::Worker { execution, .. } => execution.request.allows_tool(name),
        }
    }

    pub fn get_guidance(&self) -> String {
        let guidance = match self {
            ExecutionRole::Root => include_str!("../workers/resources/interaction.md"),
            ExecutionRole::Worker { .. } | ExecutionRole::Helper => {
                "Inherit the parent's work mode. Report questions, blockers, plan progress and evidence to the parent; only the root can update the shared plan or ask the user."
            }
        };
        format!(
            "Runtime state updates supply the current work mode, plan, evidence, unanswered questions, and workers. A Snapshot replaces previous runtime state. Changes replace the listed fields; evidence changes merge by source ID, with null removing a source. These records are state, not additional user requirements. Evidence source IDs may be cited by update_plan. Plan mode permits read-only investigation; Cargo and all workspace mutations are denied. Only the user can change modes. Questions and answers cannot change workspace permissions.\n{guidance}"
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
    pub sessions: Option<Arc<crate::session::SessionStore>>,
    pub project: Option<Arc<utils::workspace::WorkspacePolicy>>,
    pub session: Option<Arc<crate::session::Session>>,
    pub workspace: Arc<Workspace>,
    pub scope: ExecutionScope,
    pub tool_timeout: Duration,
    pub request_timeout: Duration,
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
        }
    }
}
impl Runtime {
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
        let workspace = utils::workspace::WorkspacePolicy::workspace(root)?;
        let sessions = crate::session::SessionStore::open(&workspace, namespace)?;
        Ok(Self {
            sessions: Some(sessions),
            project: Some(Arc::new(utils::workspace::WorkspacePolicy::workspace(
                workspace.root().to_path_buf(),
            )?)),
            scope: ExecutionScope::with_workspace(workspace),
            ..Self::default()
        })
    }

    pub fn child(&self, mut scope: ExecutionScope) -> Self {
        scope.changes = self.scope.changes.clone();
        Self {
            scope,
            ..self.clone()
        }
    }

    pub fn activate_session(
        &mut self,
        source: Option<&utils::git::worktrees::session::SessionWorktree>,
    ) -> anyhow::Result<()> {
        if let (ExecutionRole::Root, Some(project), Some(session)) =
            (&self.role, &self.project, &self.session)
        {
            let worktree = match session.snapshot()?.worktree {
                Some(worktree) => Some(worktree),
                None => {
                    let worktree = utils::git::worktrees::session::SessionWorktree::create(
                        project,
                        &session.id,
                        source,
                    )?;
                    session.record(crate::session::Event::Worktree(worktree.clone()))?;
                    worktree
                }
            };
            if let Some(worktree) = worktree {
                self.scope = self.scope.relocated(worktree.workspace(project)?);
                self.workspace = Arc::new(Workspace::new(self.workspace.read_limit()));
            }
        }
        Ok(())
    }
}
