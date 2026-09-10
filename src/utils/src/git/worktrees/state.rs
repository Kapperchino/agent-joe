use super::snapshot::WorktreeSnapshot;
use crate::{changes::FileVersion, git::GitRepository};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorktreeState {
    Creating,
    Active,
    Integrated {
        files: BTreeMap<PathBuf, FileVersion>,
        head: String,
    },
    Removing,
    Removed,
    Failed {
        message: String,
    },
}

#[derive(Debug)]
pub(super) enum WorktreeEvent {
    CheckoutCompleted,
    Integrated { snapshot: WorktreeSnapshot },
    RemovalStarted,
    RemovalCompleted,
    Failed { message: String },
}

impl WorktreeState {
    pub(super) fn transition(self, event: WorktreeEvent) -> anyhow::Result<Self> {
        match (self, event) {
            (Self::Creating, WorktreeEvent::CheckoutCompleted) => Ok(Self::Active),
            (Self::Active | Self::Integrated { .. }, WorktreeEvent::Integrated { snapshot }) => {
                Ok(Self::Integrated {
                    files: snapshot.files,
                    head: snapshot.head,
                })
            }
            (Self::Active | Self::Integrated { .. }, WorktreeEvent::RemovalStarted) => {
                Ok(Self::Removing)
            }
            (Self::Removing, WorktreeEvent::RemovalCompleted) => Ok(Self::Removed),
            (Self::Creating | Self::Removing, WorktreeEvent::Failed { message }) => {
                Ok(Self::Failed { message })
            }
            _ => Err(anyhow::anyhow!("Invalid managed worktree state transition")),
        }
    }

    pub(super) fn integration_files<'a>(
        &'a self,
        base: &'a WorktreeSnapshot,
    ) -> anyhow::Result<&'a BTreeMap<PathBuf, FileVersion>> {
        match self {
            Self::Active => Ok(&base.files),
            Self::Integrated { files, .. } => Ok(files),
            _ => Err(anyhow::anyhow!("Worktree is not available for integration")),
        }
    }

    pub(super) fn cleanup_snapshot(
        &self,
        git: &GitRepository,
        base: &str,
    ) -> anyhow::Result<WorktreeSnapshot> {
        match self {
            Self::Active => WorktreeSnapshot::base(git, base),
            Self::Integrated { files, head } => Ok(WorktreeSnapshot {
                files: files.clone(),
                head: head.clone(),
            }),
            _ => Err(anyhow::anyhow!(
                "Incomplete worktree operation requires inspection; automatic cleanup is unavailable"
            )),
        }
    }
}
