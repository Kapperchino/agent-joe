use super::{DiffTarget, GitRepository, Revision};
use crate::{
    changes::{ChangeTracker, FileEdit, FileVersion},
    workspace::{Access, WorkspacePolicy},
};
use serde::{Deserialize, Serialize};
use snapshot::{WorktreeSnapshot, changed_paths};
use state::WorktreeEvent;
use std::path::PathBuf;

mod snapshot;
mod state;

pub use state::WorktreeState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedWorktree {
    pub id: String,
    pub path: PathBuf,
    pub base: String,
    pub branch: String,
    pub dirty_source: bool,
    pub state: WorktreeState,
}

#[derive(Clone, Copy)]
pub enum DirtySource {
    Reject,
    BaseOnly,
}

impl DirtySource {
    pub fn new(value: &str) -> anyhow::Result<Self> {
        match value {
            "reject" => Ok(Self::Reject),
            "base_only" => Ok(Self::BaseOnly),
            _ => Err(anyhow::anyhow!(
                "Dirty source policy must be reject or base_only"
            )),
        }
    }
}

pub enum WorktreeOperation {
    Create { base: Revision, dirty: DirtySource },
    List,
    Integrate { id: String },
    Remove { id: String },
}

impl ManagedWorktree {
    pub fn execute(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        operation: WorktreeOperation,
    ) -> anyhow::Result<Vec<Self>> {
        let access = match operation {
            WorktreeOperation::List => Access::Read,
            _ => Access::Write,
        };
        let workspace = workspace
            .permits_workspace_access(access)
            .then_some(workspace)
            .ok_or_else(|| {
                anyhow::anyhow!("Worktree operations require whole-project path access")
            })?;
        match operation {
            WorktreeOperation::List => Ok(tracker.snapshot()?.worktrees),
            WorktreeOperation::Create { base, dirty } => {
                Self::create(workspace, tracker, base, dirty).map(|worktree| vec![worktree])
            }
            WorktreeOperation::Integrate { id } => Self::selected(workspace, tracker, &id)?
                .integrate(workspace, tracker)
                .map(|worktree| vec![worktree]),
            WorktreeOperation::Remove { id } => Self::selected(workspace, tracker, &id)?
                .remove(workspace, tracker)
                .map(|worktree| vec![worktree]),
        }
    }

    pub fn selected(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        id: &str,
    ) -> anyhow::Result<Self> {
        let record = tracker
            .snapshot()?
            .worktrees
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown managed worktree ID"))?;
        let uuid = uuid::Uuid::parse_str(id)?;
        let valid = record.path
            == workspace
                .root()
                .join(".joe-worktrees")
                .join(uuid.to_string())
            && record.branch == format!("joe/{uuid}");
        match valid {
            true => Ok(record),
            false => Err(anyhow::anyhow!(
                "Managed worktree path or branch does not match its saved identity"
            )),
        }
    }

    fn save(&self, tracker: &ChangeTracker) -> anyhow::Result<()> {
        let mut records = tracker.snapshot()?.worktrees;
        match records.iter().position(|record| record.id == self.id) {
            Some(index) => records[index] = self.clone(),
            None => records.push(self.clone()),
        }
        tracker.update_worktrees(records)
    }

    fn transition(mut self, tracker: &ChangeTracker, event: WorktreeEvent) -> anyhow::Result<Self> {
        self.state = self.state.transition(event)?;
        self.save(tracker)?;
        Ok(self)
    }

    fn finish(
        self,
        tracker: &ChangeTracker,
        outcome: anyhow::Result<WorktreeEvent>,
    ) -> anyhow::Result<Self> {
        match outcome {
            Ok(event) => self.transition(tracker, event),
            Err(error) => {
                self.transition(
                    tracker,
                    WorktreeEvent::Failed {
                        message: format!("{error:#}"),
                    },
                )?;
                Err(error)
            }
        }
    }

    fn create(
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
        base: Revision,
        dirty: DirtySource,
    ) -> anyhow::Result<Self> {
        let git = GitRepository::source(workspace)?;
        let checkout = WorktreeCheckout::new(workspace, &git, base, dirty)?;
        workspace.create_parent_dirs(&checkout.record.path)?;
        checkout.record.save(tracker)?;
        let outcome = checkout.execute(&git);
        checkout.record.finish(tracker, outcome)
    }

    fn child_workspace(
        &self,
        workspace: &WorkspacePolicy,
        source: &GitRepository,
    ) -> anyhow::Result<WorkspacePolicy> {
        let path = workspace
            .is_directory(&self.path)?
            .then_some(self.path.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("Managed worktree is not an ordinary project directory")
            })?;
        let child = WorkspacePolicy::workspace(path)?;
        let git = GitRepository::required(&child)?;
        let expected = source
            .repo
            .commondir()
            .join("worktrees")
            .join(&self.id)
            .canonicalize()?;
        match git.repo.path().canonicalize()? == expected {
            true => Ok(child),
            false => Err(anyhow::anyhow!("Managed worktree control metadata changed")),
        }
    }

    pub fn integration_paths(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<PathBuf>> {
        let git = GitRepository::source(workspace)?;
        let base = WorktreeSnapshot::base(&git, &self.base)?;
        let previous = self.state.integration_files(&base)?;
        let child = self.child_workspace(workspace, &git)?;
        let current = WorktreeSnapshot::current(&child, &GitRepository::required(&child)?)?;
        Ok(changed_paths(&base.files, &current.files)
            .into_iter()
            .chain(changed_paths(&base.files, previous))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    fn integrate(
        self,
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
    ) -> anyhow::Result<Self> {
        let integration = WorktreeIntegration::new(&self, workspace)?;
        let event = integration.apply(workspace, tracker)?;
        self.transition(tracker, event)
    }

    fn remove(self, workspace: &WorkspacePolicy, tracker: &ChangeTracker) -> anyhow::Result<Self> {
        let git = GitRepository::source(workspace)?;
        let removal = WorktreeRemoval::new(&self, workspace, &git)?;
        let record = self.transition(tracker, WorktreeEvent::RemovalStarted)?;
        record.finish(tracker, removal.execute())
    }
}

struct WorktreeCheckout {
    record: ManagedWorktree,
    base: WorktreeSnapshot,
}

impl WorktreeCheckout {
    fn new(
        workspace: &WorkspacePolicy,
        git: &GitRepository,
        revision: Revision,
        dirty: DirtySource,
    ) -> anyhow::Result<Self> {
        let dirty_source = !git.status(workspace)?.entries.is_empty();
        let dirty_source = match dirty {
            DirtySource::Reject if dirty_source => Err(anyhow::anyhow!(
                "Source index or worktree is dirty. Choose base_only explicitly to isolate the selected commit without copying existing edits"
            )),
            _ => Ok(dirty_source),
        }?;
        let base = WorktreeSnapshot::base(git, &git.commit(&revision)?.id().to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = workspace.root().join(".joe-worktrees").join(&id);
        base.files
            .keys()
            .try_for_each(|name| workspace.check(&path.join(name), Access::Write).map(|_| ()))?;
        Ok(Self {
            record: ManagedWorktree {
                branch: format!("joe/{id}"),
                id,
                path,
                base: base.head.clone(),
                dirty_source,
                state: WorktreeState::Creating,
            },
            base,
        })
    }

    fn execute(&self, git: &GitRepository) -> anyhow::Result<WorktreeEvent> {
        let commit = git
            .repo
            .find_commit(git2::Oid::from_str(&self.base.head)?)?;
        let branch = git.repo.branch(&self.record.branch, &commit, false)?;
        let mut options = git2::WorktreeAddOptions::new();
        options.reference(Some(branch.get()));
        git.repo
            .worktree(&self.record.id, &self.record.path, Some(&options))?;
        let child = WorkspacePolicy::workspace(self.record.path.clone())?;
        let snapshot = WorktreeSnapshot::current(&child, &GitRepository::required(&child)?)?;
        match snapshot.files == self.base.files && snapshot.head == self.base.head {
            true => Ok(WorktreeEvent::CheckoutCompleted),
            false => Err(anyhow::anyhow!(
                "Worktree checkout differs from the selected base; retained for inspection"
            )),
        }
    }
}

struct WorktreeIntegration {
    snapshot: WorktreeSnapshot,
    edits: Vec<FileEdit>,
}

impl WorktreeIntegration {
    fn new(record: &ManagedWorktree, workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        let git = GitRepository::source(workspace)?;
        let base = WorktreeSnapshot::base(&git, &record.base)?;
        let previous = record.state.integration_files(&base)?;
        match git.head()?.as_deref() == Some(&record.base) {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Integration conflict: source HEAD moved since the selected base"
            )),
        }?;
        let child = record.child_workspace(workspace, &git)?;
        let child_git = GitRepository::required(&child)?;
        let status = child_git.status(&child)?;
        let snapshot = match status.entries.iter().any(|entry| entry.conflicted) {
            true => Err(anyhow::anyhow!(
                "Resolve worktree index conflicts before integration"
            )),
            false if !child_git.diff(&child, DiffTarget::Staged, None)?.is_empty() => {
                Err(anyhow::anyhow!(
                    "Worktree has staged changes; integration preserves index state by requiring committed or unstaged worktree edits"
                ))
            }
            false => WorktreeSnapshot::current(&child, &child_git),
        }?;
        let status = git.status(workspace)?;
        let edits = changed_paths(&base.files, &snapshot.files)
            .into_iter()
            .chain(changed_paths(&base.files, previous))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|path| match status.index_conflicts_with(&path) {
                true => Err(anyhow::anyhow!(
                    "Integration conflict with source index: {}",
                    path.display()
                )),
                false => FileEdit::new(
                    workspace,
                    &path,
                    previous.get(&path).cloned().unwrap_or(FileVersion::Missing),
                    snapshot
                        .files
                        .get(&path)
                        .cloned()
                        .unwrap_or(FileVersion::Missing),
                ),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self { snapshot, edits })
    }

    fn apply(
        self,
        workspace: &WorkspacePolicy,
        tracker: &ChangeTracker,
    ) -> anyhow::Result<WorktreeEvent> {
        match self.edits.is_empty() {
            true => Ok(()),
            false => tracker.apply(workspace, self.edits).map(|_| ()),
        }?;
        Ok(WorktreeEvent::Integrated {
            snapshot: self.snapshot,
        })
    }
}

struct WorktreeRemoval<'repo> {
    branch: git2::Branch<'repo>,
    worktree: git2::Worktree,
}

impl<'repo> WorktreeRemoval<'repo> {
    fn new(
        record: &ManagedWorktree,
        workspace: &WorkspacePolicy,
        git: &'repo GitRepository,
    ) -> anyhow::Result<Self> {
        let expected = record.state.cleanup_snapshot(git, &record.base)?;
        let child = record.child_workspace(workspace, git)?;
        let child_git = GitRepository::required(&child)?;
        let actual = WorktreeSnapshot::complete(&child, &child_git)?;
        let unchanged = actual.files == expected.files
            && actual.head == expected.head
            && !child_git.repo.index()?.has_conflicts()
            && child_git.diff(&child, DiffTarget::Staged, None)?.is_empty();
        match unchanged {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Cleanup conflict: worktree has edits, commits, ignored files, or private session data that have not been integrated"
            )),
        }?;
        let branch = git
            .repo
            .find_branch(&record.branch, git2::BranchType::Local)?;
        match branch.get().target().map(|id| id.to_string()).as_ref() == Some(&expected.head) {
            true => Ok(Self {
                branch,
                worktree: git.repo.find_worktree(&record.id)?,
            }),
            false => Err(anyhow::anyhow!("Cleanup conflict: managed branch changed")),
        }
    }

    fn execute(mut self) -> anyhow::Result<WorktreeEvent> {
        let mut options = git2::WorktreePruneOptions::new();
        options.valid(true).working_tree(true);
        self.worktree.prune(Some(&mut options))?;
        self.branch.delete()?;
        Ok(WorktreeEvent::RemovalCompleted)
    }
}
