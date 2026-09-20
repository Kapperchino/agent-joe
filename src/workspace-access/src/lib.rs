use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard, Semaphore, SemaphorePermit};
use tools::{
    tool_defs::ToolEffect,
    tool_error::{ToolEffects, ToolFailure, ToolFailureKind},
};
use utils::execution::ExecutionScope;

pub use common_models::runtime_ids::WorkspaceRevision;

pub struct Workspace {
    pub writer: Arc<tokio::sync::Mutex<()>>,
    lock: RwLock<()>,
    revision: AtomicU64,
    readers: Semaphore,
    read_limit: usize,
}
impl Workspace {
    pub fn new(read_limit: usize) -> Self {
        let read_limit = read_limit.max(1);
        Self {
            writer: Arc::default(),
            lock: RwLock::new(()),
            revision: AtomicU64::new(0),
            readers: Semaphore::new(read_limit),
            read_limit,
        }
    }
    pub fn revision(&self) -> WorkspaceRevision {
        WorkspaceRevision(self.revision.load(Ordering::SeqCst))
    }
    pub fn read_limit(&self) -> usize {
        self.read_limit
    }

    pub async fn acquire(
        &self,
        effect: ToolEffect,
        scope: &ExecutionScope,
    ) -> Result<WorkspaceLease<'_>, ToolFailure> {
        tokio::select! {
            biased;
            _ = scope.cancel.cancelled() => Err(ToolFailure::new(ToolFailureKind::Cancelled, ToolEffects::NotStarted, "Cancelled while waiting for the workspace")),
            lease = self.lease(effect) => match effect {
                ToolEffect::Write | ToolEffect::Validate if scope.resources().iter().any(|resource| resource.kind == utils::execution::ResourceKind::Process) => Err(ToolFailure::new(ToolFailureKind::Validation, ToolEffects::NotStarted, "Stop the managed target with cargo operation stop before editing or running another Cargo command")),
                _ => Ok(lease),
            },
        }
    }

    async fn lease(&self, effect: ToolEffect) -> WorkspaceLease<'_> {
        match effect {
            ToolEffect::Read => WorkspaceLease::Read {
                _slot: self
                    .readers
                    .acquire()
                    .await
                    .expect("workspace semaphore is never closed"),
                _lock: self.lock.read().await,
                revision: self.revision(),
            },
            ToolEffect::Write => WorkspaceLease::Write {
                _lock: self.lock.write().await,
                revision: &self.revision,
            },
            ToolEffect::Validate => WorkspaceLease::Validate {
                _lock: self.lock.write().await,
                revision: self.revision(),
            },
            ToolEffect::Interaction
            | ToolEffect::ProcessControl
            | ToolEffect::DelegateRead
            | ToolEffect::DelegateWrite
            | ToolEffect::DelegateValidate => WorkspaceLease::Delegated,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.lock.try_write().is_ok()
    }
}

pub enum WorkspaceLease<'a> {
    Read {
        _slot: SemaphorePermit<'a>,
        _lock: RwLockReadGuard<'a, ()>,
        revision: WorkspaceRevision,
    },
    Write {
        _lock: RwLockWriteGuard<'a, ()>,
        revision: &'a AtomicU64,
    },
    Validate {
        _lock: RwLockWriteGuard<'a, ()>,
        revision: WorkspaceRevision,
    },
    Delegated,
}
impl WorkspaceLease<'_> {
    pub fn revision(&self) -> Option<WorkspaceRevision> {
        match self {
            Self::Read { revision, .. } | Self::Validate { revision, .. } => Some(*revision),
            Self::Write { revision, .. } => {
                Some(WorkspaceRevision(revision.load(Ordering::SeqCst)))
            }
            Self::Delegated => None,
        }
    }
}
impl Drop for WorkspaceLease<'_> {
    fn drop(&mut self) {
        if let Self::Write { revision, .. } = self {
            revision.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
