use crate::{Session, SessionStore};
use interaction::{access::InteractionRole, policy::InteractionPolicy};
use std::sync::{Arc, Mutex};
use tools::tool_defs::ToolOpKind;
use utils::git::worktrees::session::SessionWorktree;
use utils::{execution::ExecutionScope, workspace::WorkspacePolicy};
use workspace_access::Workspace;

#[derive(Clone)]
pub struct SessionRuntime {
    pub interaction: Arc<InteractionPolicy>,
    pub role: InteractionRole,
    pub sessions: Option<Arc<SessionStore>>,
    pub project: Option<Arc<WorkspacePolicy>>,
    pub session: Option<Arc<Session>>,
    pub workspace: Arc<Workspace>,
    pub binding: Option<Arc<SessionWorkspace>>,
    pub scope: ExecutionScope,
}

pub struct SessionWorkspace {
    project: Arc<WorkspacePolicy>,
    session: Arc<Session>,
    changes: Mutex<Arc<utils::changes::ChangeTracker>>,
}

pub struct WorkspaceCheckpoint {
    source: Option<SessionWorktree>,
    changes: Arc<utils::changes::ChangeTracker>,
}

pub enum WorkspaceRecovery {
    Unchanged,
    Retained,
}

impl SessionWorkspace {
    pub fn checkpoint(&self, effect: ToolOpKind) -> anyhow::Result<Option<WorkspaceCheckpoint>> {
        let snapshot = self.session.snapshot()?;
        match (snapshot.worktree, effect) {
            (None, ToolOpKind::Write) => Ok(Some(WorkspaceCheckpoint {
                source: snapshot.worktree_source,
                changes: self
                    .changes
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Session workspace lock poisoned"))?
                    .clone(),
            })),
            _ => Ok(None),
        }
    }

    pub fn discard_unused(
        &self,
        checkpoint: WorkspaceCheckpoint,
    ) -> anyhow::Result<WorkspaceRecovery> {
        let mut changes = self
            .changes
            .lock()
            .map_err(|_| anyhow::anyhow!("Session workspace lock poisoned"))?;
        let snapshot = changes.snapshot()?;
        match (
            self.session.snapshot()?.worktree,
            snapshot.records.is_empty(),
        ) {
            (Some(worktree), true) => {
                let base = snapshot
                    .baseline
                    .and_then(|baseline| baseline.git)
                    .and_then(|git| git.head)
                    .ok_or_else(|| anyhow::anyhow!("Missing initial worktree revision"))?;
                worktree.discard_unwritten(&self.project, &base)?;
                self.session.record(crate::Event::Worktree(None))?;
                if let Some(source) = checkpoint.source {
                    self.session.record(crate::Event::WorktreeSource(source))?;
                }
                self.session
                    .record(crate::Event::Changes(checkpoint.changes.snapshot()?))?;
                *changes = checkpoint.changes;
                Ok(WorkspaceRecovery::Unchanged)
            }
            _ => Ok(WorkspaceRecovery::Retained),
        }
    }

    fn resolve(
        &self,
        scope: &ExecutionScope,
        effect: ToolOpKind,
    ) -> anyhow::Result<ExecutionScope> {
        let mut changes = self
            .changes
            .lock()
            .map_err(|_| anyhow::anyhow!("Session workspace lock poisoned"))?;
        let snapshot = self.session.snapshot()?;
        let worktree = match (snapshot.worktree, effect) {
            (None, ToolOpKind::Write) => {
                let worktree = SessionWorktree::create(
                    &self.project,
                    &self.session.id,
                    snapshot.worktree_source.as_ref(),
                )?;
                if let Some(created) = &worktree {
                    let observed = changes.observed_versions()?;
                    self.session
                        .record(crate::Event::Worktree(worktree.clone()))?;
                    *changes = self.session.change_tracker(Default::default());
                    let workspace = created.workspace(&self.project)?;
                    changes.start(&workspace)?;
                    observed.into_iter().try_for_each(|(path, version)| {
                        match workspace.file_version(&path)? == version {
                            true => Ok(()),
                            false => Err(anyhow::anyhow!(
                                "Previously read file {} differs from the session base; no edit was applied. Reconcile the checkout with the committed session base before retrying",
                                path.display()
                            )),
                        }
                    })?;
                }
                worktree
            }
            (worktree, _) => worktree,
        };
        let workspace = match worktree.or(snapshot.worktree_source) {
            Some(worktree) => worktree.workspace(&self.project)?,
            None => WorkspacePolicy::workspace(self.project.root().to_path_buf())?,
        };
        match changes.snapshot()?.baseline {
            Some(baseline) if !workspace.matches_workspace_identity(&baseline.workspace)? => {
                *changes = self.session.change_tracker(Default::default());
                changes.start(&workspace)?;
            }
            _ => {}
        }
        let current = scope.workspace()?;
        let mut scope = match current.root() == workspace.root() {
            true => scope.clone(),
            false => scope.relocated(current.relocated(workspace.root().to_path_buf())?),
        };
        scope.changes = changes.clone();
        Ok(scope)
    }
}

impl Default for SessionRuntime {
    fn default() -> Self {
        Self {
            interaction: Arc::default(),
            role: InteractionRole::Root,
            sessions: None,
            project: None,
            session: None,
            workspace: Arc::new(Workspace::new(4)),
            binding: None,
            scope: ExecutionScope::default(),
        }
    }
}

impl SessionRuntime {
    pub fn for_workspace(root: std::path::PathBuf) -> anyhow::Result<Self> {
        Self::with_session_namespace(root, "sessions")
    }

    pub fn with_session_namespace(
        root: std::path::PathBuf,
        namespace: &str,
    ) -> anyhow::Result<Self> {
        let workspace = WorkspacePolicy::workspace(root)?;
        let sessions = SessionStore::open(&workspace, namespace)?;
        Ok(Self {
            sessions: Some(sessions),
            project: Some(Arc::new(WorkspacePolicy::workspace(
                workspace.root().to_path_buf(),
            )?)),
            scope: ExecutionScope::with_workspace(workspace),
            ..Self::default()
        })
    }

    pub fn activate_session(
        &mut self,
        source: Option<&utils::git::worktrees::session::SessionWorktree>,
    ) -> anyhow::Result<()> {
        if let (InteractionRole::Root, Some(project), Some(session)) =
            (&self.role, &self.project, &self.session)
        {
            if let Some(source) = source {
                session.record(crate::Event::WorktreeSource(source.clone()))?;
            }
            self.binding = Some(Arc::new(SessionWorkspace {
                project: project.clone(),
                session: session.clone(),
                changes: Mutex::new(self.scope.changes.clone()),
            }));
        }
        self.prepare_workspace(ToolOpKind::Read)
    }

    pub fn prepare_workspace(&mut self, effect: ToolOpKind) -> anyhow::Result<()> {
        if let Some(binding) = &self.binding {
            self.scope = binding.resolve(&self.scope, effect)?;
        }
        Ok(())
    }
}
