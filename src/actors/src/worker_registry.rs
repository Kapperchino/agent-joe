pub mod budget;
mod launch;
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
use utils::execution::ExecutionScope;

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
    tokens: usize,
}

impl Allocation {
    fn reserve(&self, active: usize, budget: request::BudgetLimits) -> anyhow::Result<Self> {
        match active < 4 && self.workers < 32 && self.tokens + budget.tokens() <= 2_000_000 {
            true => Ok(Self {
                workers: self.workers + 1,
                tokens: self.tokens + budget.tokens(),
            }),
            false => Err(anyhow::anyhow!(
                "Worker limit exhausted: at most 4 active workers, 32 starts and 2000000 allocated tokens per session"
            )),
        }
    }
}

struct Entry {
    owner: String,
    id: String,
    request: WorkerRequest,
    scope: ExecutionScope,
    updates: watch::Sender<WorkerState>,
}

pub struct WorkerExecution {
    pub id: String,
    pub request: WorkerRequest,
    pub budget: Arc<WorkerBudget>,
    evidence: Mutex<Evidence>,
    pub(crate) session: Mutex<Option<Arc<crate::session::Session>>>,
}

impl WorkerExecution {
    pub(crate) fn attach_session(
        &self,
        session: Option<Arc<crate::session::Session>>,
    ) -> anyhow::Result<()> {
        if let Some(session) = &session {
            self.evidence.lock().unwrap().inherited_artifacts = session
                .snapshot()?
                .artifacts
                .into_iter()
                .map(|artifact| artifact.id)
                .collect();
        }
        *self.session.lock().unwrap() = session;
        Ok(())
    }
    pub(crate) fn record(
        &self,
        effect: tools::tool_defs::ToolEffect,
        result: &tools::tool_defs::ToolResult,
    ) {
        self.evidence.lock().unwrap().record(effect, result);
    }
}

impl WorkerRegistry {
    fn register(
        &self,
        owner: &str,
        scope: ExecutionScope,
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
            .reserve(active, request.budget)?;
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
                scope,
                updates,
            },
        );
        Ok(Arc::new(WorkerExecution {
            id,
            budget: Arc::new(WorkerBudget::new(request.budget)),
            request,
            evidence: Mutex::new(Evidence::default()),
            session: Mutex::new(None),
        }))
    }

    pub(crate) fn restore(&self, owner: &str, workers: BTreeMap<String, WorkerView>) {
        let mut state = self.state.lock().unwrap();
        let allocation = Allocation {
            workers: workers.len(),
            tokens: workers
                .values()
                .map(|worker| worker.request.budget.tokens())
                .sum(),
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
                    scope: ExecutionScope::default(),
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
            entry.scope.cancel.cancel();
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

    pub(crate) fn pending(&self, owner: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.owner == owner && entry.updates.borrow().pending())
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

    fn running(&self, owner: &str, id: &str) {
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

    fn complete(&self, owner: &str, report: WorkerReport) {
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
#[path = "../../../tests/actors/worker_registry/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/actors/worker_registry/recovery_tests.rs"]
mod recovery_tests;
