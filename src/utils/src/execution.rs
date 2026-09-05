use std::{
    collections::BTreeMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone, Default)]
pub struct ExecutionScope {
    pub cancel: CancellationToken,
    pub tasks: TaskTracker,
    resources: Arc<Mutex<BTreeMap<u64, Resource>>>,
}

tokio::task_local! { static CURRENT: ExecutionScope; }

impl ExecutionScope {
    pub fn child(&self) -> Self {
        Self {
            cancel: self.cancel.child_token(),
            tasks: TaskTracker::new(),
            resources: self.resources.clone(),
        }
    }

    pub fn current() -> Self {
        CURRENT.try_with(Clone::clone).unwrap_or_default()
    }

    pub async fn enter<F: Future>(&self, future: F) -> F::Output {
        CURRENT.scope(self.clone(), future).await
    }

    pub async fn finish(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Provider,
    Tool,
    Worker,
    Process,
}
#[derive(Debug, Clone)]
pub struct Resource {
    pub id: u64,
    pub kind: ResourceKind,
    pub description: String,
}

pub struct Registration {
    id: u64,
    resources: Arc<Mutex<BTreeMap<u64, Resource>>>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.resources.lock().unwrap().remove(&self.id);
    }
}
impl ExecutionScope {
    pub fn register(&self, kind: ResourceKind, description: String) -> Registration {
        let id = next_id();
        self.resources.lock().unwrap().insert(
            id,
            Resource {
                id,
                kind,
                description,
            },
        );
        Registration {
            id,
            resources: self.resources.clone(),
        }
    }
    pub fn resources(&self) -> Vec<Resource> {
        self.resources.lock().unwrap().values().cloned().collect()
    }
}

pub struct OwnedScope(ExecutionScope);
impl OwnedScope {
    pub fn new(scope: ExecutionScope) -> Self {
        Self(scope)
    }
}
impl std::ops::Deref for OwnedScope {
    type Target = ExecutionScope;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for OwnedScope {
    fn drop(&mut self) {
        self.0.cancel.cancel();
    }
}
