use super::{Event, Session, SessionStore};
use anyhow::Context;
use std::sync::Arc;
use utils::{git::worktrees::session::PruneOutcome, workspace::WorkspacePolicy};

impl SessionStore {
    pub(crate) fn prune_worktrees(
        self: &Arc<Self>,
        project: &WorkspacePolicy,
    ) -> anyhow::Result<String> {
        let identity = project.workspace_identity()?;
        match identity == self.storage.workspace_identity() {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Session storage does not belong to the current workspace"
            )),
        }?;
        let mut removed = 0;
        let mut skipped = 0;
        let rows = self
            .list()?
            .into_iter()
            .filter(|snapshot| snapshot.parent.is_none() && snapshot.worktree.is_some())
            .map(
                |snapshot| match self.prune_worktree(&snapshot.id, project) {
                    Ok(PruneOutcome::Pruned) => {
                        removed += 1;
                        format!("Pruned {}", snapshot.id)
                    }
                    Ok(PruneOutcome::Merged) => {
                        skipped += 1;
                        format!("Skipped {}: no unmerged changes", snapshot.id)
                    }
                    Err(error) => {
                        skipped += 1;
                        format!("Skipped {}: {error:#}", snapshot.id)
                    }
                },
            )
            .collect::<Vec<_>>();
        Ok(format!(
            "Pruned {removed} unmerged session worktree(s); skipped {skipped}.\n{}\nDiscarded worktree changes cannot be restored by /resume. Saved conversations remain; resuming a pruned session creates a fresh worktree from main.",
            rows.join("\n")
        ))
    }

    fn prune_worktree(
        self: &Arc<Self>,
        id: &str,
        project: &WorkspacePolicy,
    ) -> anyhow::Result<PruneOutcome> {
        let owner = self.update(Some(id), |database| {
            let mut transaction = database.env.write_txn()?;
            let snapshot = database.snapshot(&transaction, id)?;
            match snapshot.workspace == self.storage.workspace_identity()
                && snapshot.parent.is_none()
            {
                true => Ok(()),
                false => Err(anyhow::anyhow!("Not a root session in this project")),
            }?;
            let owner = database.claim(&mut transaction, id)?;
            transaction.commit()?;
            Ok(owner)
        })?;
        let session = Session {
            store: self.clone(),
            id: id.to_owned(),
            owner,
        };
        let worktree = session
            .snapshot()?
            .worktree
            .ok_or_else(|| anyhow::anyhow!("Session no longer has a worktree"))?;
        match worktree.id() == id {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Session worktree ID does not match its owner"
            )),
        }?;
        let outcome = worktree.prune(project)?;
        if outcome == PruneOutcome::Pruned {
            session.record(Event::WorktreePruned).context(
                "Worktree was removed but session metadata could not be updated; retry /prune",
            )?;
        }
        Ok(outcome)
    }
}
