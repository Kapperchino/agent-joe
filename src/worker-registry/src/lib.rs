pub mod budget;
pub mod report;
pub mod request;
mod state;

use budget::WorkerBudget;
use report::{Evidence, WorkerReport, WorkerStatus, WorkerView};
use request::WorkerRequest;
use state::{WorkerState, WorkerUpdate};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct WorkerRegistry {
    state: Mutex<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    next_id: u64,
    entries: BTreeMap<String, Entry>,
    allocations: BTreeMap<String, Allocation>,
}

#[derive(Default)]
struct Allocation {
    workers: usize,
}

impl Allocation {
    fn reserve(&self, active: usize) -> anyhow::Result<Self> {
        match (active, self.workers) {
            (0..4, 0..32) => Ok(Self {
                workers: self.workers + 1,
            }),
            _ => Err(anyhow::anyhow!(
                "Worker limit exhausted: at most 4 active workers and 32 starts per session"
            )),
        }
    }
}

struct Entry {
    owner: String,
    id: String,
    request: WorkerRequest,
    cancel: CancellationToken,
    updates: watch::Sender<WorkerState>,
}

pub struct WorkerExecution {
    pub id: String,
    pub request: WorkerRequest,
    pub budget: Arc<WorkerBudget>,
    evidence: Mutex<Evidence>,
}

impl WorkerExecution {
    pub fn record(
        &self,
        effect: tools::tool_defs::ToolOpKind,
        result: &tools::tool_defs::ToolResult,
    ) {
        self.evidence.lock().unwrap().record(effect, result);
    }
}

impl WorkerRegistry {
    pub fn register(
        &self,
        owner: &str,
        cancel: CancellationToken,
        request: WorkerRequest,
    ) -> anyhow::Result<Arc<WorkerExecution>> {
        let mut state = self.state.lock().unwrap();
        let active = state
            .entries
            .values()
            .filter(|entry| !entry.updates.borrow().terminal())
            .count();
        let allocation = state
            .allocations
            .get(owner)
            .unwrap_or(&Allocation::default())
            .reserve(active)?;
        state.allocations.insert(owner.to_owned(), allocation);
        state.next_id += 1;
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let id = format!("worker-{epoch}-{}", state.next_id);
        let (updates, _) = watch::channel(WorkerState::Registered);
        state.entries.insert(
            format!("{owner}/{id}"),
            Entry {
                owner: owner.to_owned(),
                id: id.clone(),
                request: request.clone(),
                cancel,
                updates,
            },
        );
        Ok(Arc::new(WorkerExecution {
            id,
            budget: Arc::new(WorkerBudget::default()),
            request,
            evidence: Mutex::new(Evidence::default()),
        }))
    }

    pub fn restore(&self, owner: &str, workers: BTreeMap<String, WorkerView>) {
        let mut state = self.state.lock().unwrap();
        let allocation = Allocation {
            workers: workers.len(),
        };
        state.allocations.insert(owner.to_owned(), allocation);
        state.entries.retain(|_, entry| entry.owner != owner);
        state.entries.extend(workers.into_values().map(|view| {
            let (updates, _) = watch::channel(WorkerState::restored(&view));
            (
                format!("{owner}/{}", view.worker_id),
                Entry {
                    owner: owner.to_owned(),
                    id: view.worker_id,
                    request: view.request,
                    cancel: CancellationToken::default(),
                    updates,
                },
            )
        }));
    }

    pub fn list(&self, owner: &str) -> Vec<WorkerView> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.owner == owner)
            .map(Entry::view)
            .collect()
    }

    pub fn status(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let state = self.state.lock().unwrap();
        Ok(Self::entry(&state, owner, id)?.collect())
    }

    pub fn collect(&self, owner: &str) -> Vec<WorkerView> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.owner == owner)
            .map(Entry::collect)
            .collect()
    }

    pub async fn wait(&self, owner: &str, id: &str, seconds: u64) -> anyhow::Result<WorkerView> {
        let mut updates = {
            let state = self.state.lock().unwrap();
            Self::entry(&state, owner, id)?.updates.subscribe()
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(seconds.min(60)), async {
            while !updates.borrow_and_update().terminal() {
                updates.changed().await?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
        self.status(owner, id)
    }

    pub fn cancel(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let state = self.state.lock().unwrap();
        let entry = Self::entry(&state, owner, id)?;
        if entry.update(WorkerUpdate::Cancelled) {
            entry.cancel.cancel();
        }
        Ok(entry.view())
    }

    pub fn cleanup(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let mut state = self.state.lock().unwrap();
        let view = Self::entry(&state, owner, id)?.view();
        match view.status.terminal() {
            true => {
                state.entries.remove(&format!("{owner}/{id}"));
                Ok(view)
            }
            false => Err(anyhow::anyhow!(
                "Cancel and wait for worker cleanup before removing its registry entry"
            )),
        }
    }

    pub fn pending(&self, owner: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.owner == owner)
            .filter(|entry| entry.updates.borrow().pending())
            .map(|entry| {
                format!(
                    "{}: {:?}; retrieve its report with worker_status",
                    entry.id,
                    entry.updates.borrow().status()
                )
            })
            .collect()
    }

    fn entry<'a>(state: &'a RegistryState, owner: &str, id: &str) -> anyhow::Result<&'a Entry> {
        state
            .entries
            .get(&format!("{owner}/{id}"))
            .filter(|entry| entry.owner == owner)
            .ok_or_else(|| anyhow::anyhow!("Unknown worker {id} in this session"))
    }

    pub fn running(&self, owner: &str, id: &str) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap()
            .entries
            .get(&format!("{owner}/{id}"))
        {
            entry.update(WorkerUpdate::Started);
        }
    }

    pub fn complete(&self, owner: &str, report: WorkerReport) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap()
            .entries
            .get(&format!("{owner}/{}", report.worker_id))
        {
            entry.update(WorkerUpdate::Finished(Box::new(report)));
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/recovery_tests.rs"]
mod recovery_tests;
