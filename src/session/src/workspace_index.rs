use super::{SchemaVersion, Snapshot};
use heed::{
    Database, Env, RoTxn, RwTxn,
    types::{Bytes, Str},
};
use serde::{Deserialize, Serialize};
use utils::git::worktrees::session::SessionWorktree;

const DATABASE: &str = "session_workspaces";

pub(super) struct WorkspaceIndex {
    entries: Database<Str, Bytes>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct WorkspaceSnapshot {
    version: SchemaVersion,
    pub worktree: Option<SessionWorktree>,
    pub worktree_source: Option<SessionWorktree>,
}

struct WorkspaceEntry {
    id: String,
    snapshot: WorkspaceSnapshot,
}

impl From<&Snapshot> for WorkspaceSnapshot {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            version: SchemaVersion,
            worktree: snapshot.worktree.clone(),
            worktree_source: snapshot.worktree_source.clone(),
        }
    }
}

impl WorkspaceIndex {
    pub fn open(
        env: &Env,
        transaction: &mut RwTxn<'_>,
        snapshots: Database<Str, Bytes>,
    ) -> anyhow::Result<Self> {
        match env.open_database(transaction, Some(DATABASE))? {
            Some(entries) => Ok(Self { entries }),
            None => {
                let index = Self {
                    entries: env.create_database(transaction, Some(DATABASE))?,
                };
                let workspaces = snapshots
                    .iter(transaction)?
                    .map(|entry| {
                        let (id, bytes) = entry?;
                        Ok(WorkspaceEntry {
                            id: id.to_owned(),
                            snapshot: super::decode(bytes)?,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                workspaces.iter().try_for_each(|workspace| {
                    index
                        .entries
                        .put(
                            transaction,
                            &workspace.id,
                            &serde_json::to_vec(&workspace.snapshot)?,
                        )
                        .map_err(anyhow::Error::from)
                })?;
                Ok(index)
            }
        }
    }

    pub fn record(&self, transaction: &mut RwTxn<'_>, snapshot: &Snapshot) -> anyhow::Result<()> {
        self.entries.put(
            transaction,
            &snapshot.id,
            &serde_json::to_vec(&WorkspaceSnapshot::from(snapshot))?,
        )?;
        Ok(())
    }

    pub fn get(&self, transaction: &RoTxn<'_>, id: &str) -> anyhow::Result<WorkspaceSnapshot> {
        let bytes = self
            .entries
            .get(transaction, id)?
            .ok_or_else(|| anyhow::anyhow!("Session {id} workspace does not exist"))?;
        super::decode(bytes)
    }
}

#[cfg(test)]
#[path = "../tests/unit/workspace_index_test.rs"]
mod tests;
