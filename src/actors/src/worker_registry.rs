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

    #[test]
    fn cancellation_and_late_updates_preserve_reports_until_collection() {
        let registry = WorkerRegistry::default();
        let scope = ExecutionScope::default();
        let worker = registry
            .register("parent", scope.clone(), request(1024))
            .unwrap();
        assert_eq!(
            registry.status("parent", &worker.id).unwrap().status,
            WorkerStatus::Registered
        );
        assert_eq!(registry.pending("parent").len(), 1);
        registry.cancel("parent", &worker.id).unwrap();
        registry.running("parent", &worker.id);
        assert!(scope.cancel.is_cancelled());
        assert_eq!(registry.list("parent")[0].status, WorkerStatus::Cancelling);
        assert!(registry.cleanup("parent", &worker.id).is_err());
        registry.complete(
            "parent",
            worker.report(WorkerOutcome::Cancelled, std::time::Instant::now()),
        );
        registry.cancel("parent", &worker.id).unwrap();
        registry.running("parent", &worker.id);
        registry.complete(
            "parent",
            worker.report(
                WorkerOutcome::Failed("late failure".into()),
                std::time::Instant::now(),
            ),
        );
        assert_eq!(registry.list("parent")[0].status, WorkerStatus::Cancelled);
        assert_eq!(registry.pending("parent").len(), 1);
        let collected = registry.status("parent", &worker.id).unwrap();
        assert_eq!(collected.status, WorkerStatus::Cancelled);
        assert_eq!(
            collected.report.unwrap().findings,
            "Worker cancelled after cleanup"
        );
        assert!(registry.pending("parent").is_empty());
        registry.running("parent", &worker.id);
        registry.complete(
            "parent",
            worker.report(
                WorkerOutcome::Completed("late success".into()),
                std::time::Instant::now(),
            ),
        );
        assert!(registry.pending("parent").is_empty());
        assert_eq!(
            registry.cleanup("parent", &worker.id).unwrap().status,
            WorkerStatus::Cancelled
        );
    }

    #[test]
    fn recovery_requires_a_matching_terminal_report_and_preserves_completed_evidence() {
        let registry = WorkerRegistry::default();
        let worker = registry
            .register("parent", ExecutionScope::default(), request(1024))
            .unwrap();
        let report = worker.report(
            WorkerOutcome::Completed("Saved evidence".into()),
            std::time::Instant::now(),
        );
        let completed = WorkerView {
            worker_id: worker.id.clone(),
            request: worker.request.clone(),
            status: WorkerStatus::Completed,
            report: Some(report.clone()),
        };
        for mut saved in [
            WorkerView {
                report: None,
                ..completed.clone()
            },
            WorkerView {
                status: WorkerStatus::Running,
                ..completed.clone()
            },
            WorkerView {
                report: Some(WorkerReport {
                    worker_id: "another-worker".into(),
                    ..report
                }),
                ..completed.clone()
            },
        ] {
            saved.recover();
            assert_eq!(saved.status, WorkerStatus::Interrupted);
            assert_eq!(saved.report.as_ref().unwrap().worker_id, worker.id);
            assert!(saved.report.as_ref().unwrap().unresolved_issues[0].contains("uncertain"));
            saved.recover();
            assert_eq!(saved.status, WorkerStatus::Interrupted);
        }
        let encoded = serde_json::to_value(&completed).unwrap();
        let mut restored: WorkerView = serde_json::from_value(encoded.clone()).unwrap();
        restored.recover();
        assert_eq!(serde_json::to_value(&restored).unwrap(), encoded);
        registry.restore("resumed", BTreeMap::from([(worker.id.clone(), restored)]));
        assert_eq!(
            registry.list("resumed")[0]
                .report
                .as_ref()
                .unwrap()
                .findings,
            "Saved evidence"
        );
        assert!(registry.pending("resumed").is_empty());
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
