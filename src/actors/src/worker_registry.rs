pub mod budget;
mod launch;
pub mod report;
pub mod request;

use budget::WorkerBudget;
use report::{Evidence, WorkerReport, WorkerStatus, WorkerView};
use request::WorkerRequest;
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

struct Entry {
    owner: String,
    scope: ExecutionScope,
    updates: watch::Sender<WorkerView>,
    observed: bool,
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
            .filter(|entry| !entry.updates.borrow().status.terminal())
            .count();
        let allocation = state.allocations.entry(owner.to_owned()).or_default();
        match active < 4
            && allocation.workers < 32
            && allocation.tokens + request.budget.tokens() <= 2_000_000
        {
            true => {
                allocation.workers += 1;
                allocation.tokens += request.budget.tokens();
                state.next_id += 1;
                let epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let id = format!("worker-{epoch}-{}", state.next_id);
                let view = WorkerView {
                    worker_id: id.clone(),
                    request: request.clone(),
                    status: WorkerStatus::Registered,
                    report: None,
                };
                let (updates, _) = watch::channel(view);
                state.entries.insert(
                    format!("{owner}/{id}"),
                    Entry {
                        owner: owner.to_owned(),
                        scope,
                        updates,
                        observed: false,
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
            false => Err(anyhow::anyhow!(
                "Worker limit exhausted: at most 4 active workers, 32 starts and 2000000 allocated tokens per session"
            )),
        }
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
        for (id, view) in workers {
            let (updates, _) = watch::channel(view);
            state.entries.insert(
                format!("{owner}/{id}"),
                Entry {
                    owner: owner.to_owned(),
                    scope: ExecutionScope::default(),
                    updates,
                    observed: true,
                },
            );
        }
    }

    pub fn list(&self, owner: &str) -> Vec<WorkerView> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|entry| entry.owner == owner)
            .map(|entry| entry.updates.borrow().clone())
            .collect()
    }

    pub fn status(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let mut state = self.state.lock().unwrap();
        let entry = Self::entry(&mut state, owner, id)?;
        let view = entry.updates.borrow().clone();
        entry.observed = view.status.terminal();
        Ok(view)
    }

    pub fn collect(&self, owner: &str) -> Vec<WorkerView> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values_mut()
            .filter(|entry| entry.owner == owner)
            .map(|entry| {
                let view = entry.updates.borrow().clone();
                entry.observed = view.status.terminal();
                view
            })
            .collect()
    }

    pub async fn wait(&self, owner: &str, id: &str, seconds: u64) -> anyhow::Result<WorkerView> {
        let mut updates = {
            let mut state = self.state.lock().unwrap();
            Self::entry(&mut state, owner, id)?.updates.subscribe()
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(seconds.min(60)), async {
            while !updates.borrow_and_update().status.terminal() {
                updates.changed().await?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
        self.status(owner, id)
    }

    pub fn cancel(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let mut state = self.state.lock().unwrap();
        let entry = Self::entry(&mut state, owner, id)?;
        if !entry.updates.borrow().status.terminal() {
            entry
                .updates
                .send_modify(|view| view.status = WorkerStatus::Cancelling);
            entry.scope.cancel.cancel();
        }
        let view = entry.updates.borrow().clone();
        Ok(view)
    }

    pub fn cleanup(&self, owner: &str, id: &str) -> anyhow::Result<WorkerView> {
        let mut state = self.state.lock().unwrap();
        let view = Self::entry(&mut state, owner, id)?.updates.borrow().clone();
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
            .iter()
            .filter(|(_, entry)| entry.owner == owner && !entry.observed)
            .map(|(_, entry)| {
                let view = entry.updates.borrow();
                format!(
                    "{}: {:?}; retrieve its report with worker_status",
                    view.worker_id, view.status
                )
            })
            .collect()
    }

    fn entry<'a>(
        state: &'a mut RegistryState,
        owner: &str,
        id: &str,
    ) -> anyhow::Result<&'a mut Entry> {
        state
            .entries
            .get_mut(&format!("{owner}/{id}"))
            .filter(|entry| entry.owner == owner)
            .ok_or_else(|| anyhow::anyhow!("Unknown worker {id} in this session"))
    }

    fn running(&self, owner: &str, id: &str) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap()
            .entries
            .get_mut(&format!("{owner}/{id}"))
            && entry.updates.borrow().status == WorkerStatus::Registered
        {
            entry
                .updates
                .send_modify(|view| view.status = WorkerStatus::Running);
        }
    }

    fn complete(&self, owner: &str, report: WorkerReport) {
        if let Some(entry) = self
            .state
            .lock()
            .unwrap()
            .entries
            .get_mut(&format!("{owner}/{}", report.worker_id))
            && !entry.updates.borrow().status.terminal()
        {
            entry.updates.send_modify(|view| {
                view.status = report.status;
                view.report = Some(report.clone());
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launch::WorkerOutcome;
    use request::WorkerRequestInput;

    fn request(tokens: usize) -> WorkerRequest {
        WorkerRequest::new(
            WorkerRequestInput {
                objective: "Investigate a bounded question".into(),
                allowed_tools: "read_file".into(),
                allowed_paths: ".".into(),
                completion_criteria: "Report evidence".into(),
                tokens: Some(tokens),
                ..Default::default()
            },
            |_| Some(tools::tool_defs::ToolEffect::Read),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn registry_retains_immediate_completions_and_enforces_owner_count_and_total_budgets() {
        let registry = WorkerRegistry::default();
        let workers = (0..4)
            .map(|_| {
                registry
                    .register("parent", ExecutionScope::default(), request(1024))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(
            registry
                .register("parent", ExecutionScope::default(), request(1024))
                .is_err()
        );
        assert!(registry.status("other", &workers[0].id).is_err());
        assert!(registry.cancel("other", &workers[0].id).is_err());
        assert!(registry.cleanup("parent", &workers[0].id).is_err());
        for worker in &workers {
            registry.complete(
                "parent",
                worker.report(
                    WorkerOutcome::Completed("immediate result".into()),
                    std::time::Instant::now(),
                ),
            );
            let view = registry.wait("parent", &worker.id, 0).await.unwrap();
            assert_eq!(view.report.unwrap().findings, "immediate result");
            registry.cleanup("parent", &worker.id).unwrap();
        }
        assert!(registry.pending("parent").is_empty());
        for _ in 4..32 {
            let worker = registry
                .register("parent", ExecutionScope::default(), request(1024))
                .unwrap();
            registry.complete(
                "parent",
                worker.report(
                    WorkerOutcome::Completed(String::new()),
                    std::time::Instant::now(),
                ),
            );
            registry.cleanup("parent", &worker.id).unwrap();
        }
        assert!(
            registry
                .register("parent", ExecutionScope::default(), request(1024))
                .is_err()
        );
        for _ in 0..4 {
            let worker = registry
                .register("budget-parent", ExecutionScope::default(), request(500_000))
                .unwrap();
            registry.complete(
                "budget-parent",
                worker.report(
                    WorkerOutcome::Completed(String::new()),
                    std::time::Instant::now(),
                ),
            );
        }
        assert!(
            registry
                .register("budget-parent", ExecutionScope::default(), request(1024))
                .is_err()
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn recovery_exposes_uncertain_work_and_preserves_limits_across_forks() {
        let request = WorkerRequest::new(
            request::WorkerRequestInput {
                objective: "Investigate".into(),
                allowed_tools: "read_file".into(),
                allowed_paths: ".".into(),
                completion_criteria: "Report evidence".into(),
                tokens: Some(500_000),
                ..Default::default()
            },
            |_| Some(tools::tool_defs::ToolEffect::Read),
        )
        .unwrap();
        let mut view = WorkerView {
            worker_id: "saved-worker".into(),
            request: request.clone(),
            status: WorkerStatus::Running,
            report: None,
        };
        view.recover();
        assert_eq!(view.status, WorkerStatus::Interrupted);
        assert!(view.report.as_ref().unwrap().unresolved_issues[0].contains("uncertain"));
        let registry = WorkerRegistry::default();
        let saved = (0..4)
            .map(|index| {
                let id = format!("saved-{index}");
                (
                    id.clone(),
                    WorkerView {
                        worker_id: id,
                        ..view.clone()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        registry.restore("source", saved.clone());
        registry.restore("fork", saved);
        assert_eq!(registry.list("source").len(), 4);
        assert_eq!(registry.list("fork").len(), 4);
        assert!(
            registry
                .register("source", ExecutionScope::default(), request.clone())
                .is_err()
        );
        assert!(
            registry
                .register("fork", ExecutionScope::default(), request)
                .is_err()
        );
        registry.cleanup("fork", "saved-0").unwrap();
        assert!(registry.status("source", "saved-0").is_ok());
    }
}
