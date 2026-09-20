use crate::{Session, SessionStore};
use interaction::{access::InteractionRole, policy::InteractionPolicy};
use std::sync::Arc;
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
    pub scope: ExecutionScope,
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
            let worktree = match session.snapshot()?.worktree {
                Some(worktree) => Some(worktree),
                None => {
                    let worktree = utils::git::worktrees::session::SessionWorktree::create(
                        project,
                        &session.id,
                        source,
                    )?;
                    session.record(crate::Event::Worktree(worktree.clone()))?;
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
