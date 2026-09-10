use super::{
    WorkerExecution, WorkerRegistry,
    budget::BudgetState,
    report::{WorkerReport, WorkerStatus, WorkerView},
    request::{WorkerRequest, WorkerRole},
};
use crate::{
    actor::{ActorInfo, Dependency},
    runtime::ExecutionRole,
    worker::{Worker, WorkerFailure, run_worker},
    workers::task_worker::TaskWorker,
};
use analysis::contexts::{context::Context, rust_context::RustContext};
use futures::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use utils::execution::ExecutionScope;

struct PreparedWorker {
    parent_scope: ExecutionScope,
    scope: ExecutionScope,
    dependency: Dependency<RustContext>,
    writer: Option<tokio::sync::OwnedMutexGuard<()>>,
}

pub(super) enum WorkerOutcome {
    Completed(String),
    Failed(String),
    Cancelled,
    TimedOut,
}

impl WorkerOutcome {
    fn status(&self) -> WorkerStatus {
        match self {
            Self::Completed(_) => WorkerStatus::Completed,
            Self::Failed(_) => WorkerStatus::Failed,
            Self::Cancelled => WorkerStatus::Cancelled,
            Self::TimedOut => WorkerStatus::TimedOut,
        }
    }

    fn findings(self) -> String {
        match self {
            Self::Completed(findings) | Self::Failed(findings) => findings,
            Self::Cancelled => "Worker cancelled after cleanup".into(),
            Self::TimedOut => {
                "Worker deadline exceeded; owned work was cancelled and cleaned up".into()
            }
        }
    }
}

impl PreparedWorker {
    fn new(
        info: &ActorInfo<RustContext>,
        context: &RustContext,
        request: &WorkerRequest,
    ) -> anyhow::Result<Self> {
        let effect = match request.role {
            WorkerRole::Read => tools::tool_defs::ToolEffect::DelegateRead,
            WorkerRole::Write => tools::tool_defs::ToolEffect::DelegateWrite,
        };
        info.dep.runtime.interaction.authorize(effect)?;
        let parent_scope = match &info.dep.runtime.role {
            ExecutionRole::Root => info
                .dep
                .runtime
                .turn_scope
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Workers require an active parent turn")),
            ExecutionRole::Worker { .. } => Err(anyhow::anyhow!("Maximum worker depth is one")),
            ExecutionRole::Helper => Err(anyhow::anyhow!("Only the root can start workers")),
        }?;
        let access = match request.role {
            WorkerRole::Read => utils::workspace::RootAccess::ReadOnly,
            WorkerRole::Write => utils::workspace::RootAccess::ReadWrite,
        };
        let scope = parent_scope.restricted_child(&request.allowed_paths, access)?;
        let workspace = scope.workspace()?;
        let tools = request
            .allowed_tools
            .iter()
            .map(|name| {
                let tool = info.dep
                    .tool(name)
                    .filter(|tool| !tool.effect().delegates() && !matches!(name.as_str(), "update_plan" | "request_user_input"))
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Tool {name} is not permitted by the parent and registered worker tools"
                        )
                    })?;
                let access = match name.as_str() {
                    "cargo" | "worktree" => Some(utils::workspace::Access::Write),
                    "inspect_context" | "git" | "review_changes" => Some(utils::workspace::Access::Read),
                    _ if tool.effect() == tools::tool_defs::ToolEffect::Validate => Some(utils::workspace::Access::Write),
                    _ => None,
                };
                match access.is_none_or(|access| workspace.permits_workspace_access(access)) {
                    true => Ok(tool),
                    false => Err(anyhow::anyhow!(
                        "Repository inspection, worktree management and Cargo require whole-project paths; use scoped file tools or ask the root to review and validate"
                    )),
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let context = RustContext {
            initial_prompt: format!(
                "{}\n\nParent operating instructions:\n{}",
                TaskWorker::init_prompt(None),
                context.instructions()
            ),
            task_prompt: Some(request.prompt(&info.dep.runtime.inherited_constraints)?),
            guidance: context.guidance.fork(),
            id_gen: Arc::new(std::sync::atomic::AtomicU64::new(info.dep.context.gen_id())),
            ..context.clone()
        };
        let writer = match request.role {
            WorkerRole::Write => Some(info.dep.runtime.workspace.writer.clone().try_lock_owned().map_err(|_| anyhow::anyhow!("A writer already owns the workspace; wait before starting another write worker"))?),
            WorkerRole::Read => None,
        };
        let runtime = crate::runtime::Runtime {
            turn_scope: None,
            ..info.dep.runtime.child(scope.clone())
        };
        Ok(Self {
            parent_scope,
            scope,
            dependency: Dependency {
                client: info.dep.client.clone(),
                tui_tx: info.dep.tui_tx.clone(),
                debug_mode: info.dep.debug_mode,
                context,
                tools,
                runtime,
            },
            writer,
        })
    }
}

impl WorkerRegistry {
    pub(crate) fn start(
        self: &Arc<Self>,
        info: &ActorInfo<RustContext>,
        context: &RustContext,
        request: WorkerRequest,
    ) -> anyhow::Result<WorkerView> {
        let mut prepared = PreparedWorker::new(info, context, &request)?;
        let owner = info.dep.worker_owner();
        let execution = self.register(&owner, prepared.scope.clone(), request)?;
        let initial = WorkerView {
            worker_id: execution.id.clone(),
            request: execution.request.clone(),
            status: WorkerStatus::Registered,
            report: None,
        };
        let parent_session = info.dep.runtime.session.clone();
        parent_session
            .as_ref()
            .map(|session| session.record(crate::session::Event::Worker(Box::new(initial.clone()))))
            .transpose()
            .map_err(|error| {
                self.complete(
                    &owner,
                    execution.report(
                        WorkerOutcome::Failed(format!(
                            "Worker registration persistence failed: {error}"
                        )),
                        Instant::now(),
                    ),
                );
                error
            })?;
        prepared.dependency.runtime.role = ExecutionRole::Worker {
            execution: execution.clone(),
        };
        let parent = info.actor_ref.clone();
        let registry = self.clone();
        prepared.parent_scope.tasks.clone().spawn(async move {
            let runner = prepared.parent_scope.child();
            let started = Instant::now();
            registry.running(&owner, &execution.id);
            let result = AssertUnwindSafe(async {
                tokio::time::timeout(
                    Duration::from_secs(execution.request.budget.seconds()),
                    runner.enter(run_worker(TaskWorker, prepared.dependency, parent)),
                )
                .await
            })
            .catch_unwind()
            .await;
            runner.finish().await;
            prepared.scope.finish().await;
            let outcome = match result {
                Ok(Ok(Ok(text))) => WorkerOutcome::Completed(text),
                Ok(Ok(Err(WorkerFailure::Cancelled))) => WorkerOutcome::Cancelled,
                Ok(Ok(Err(error))) => WorkerOutcome::Failed(error.to_string()),
                Ok(Err(_)) => WorkerOutcome::TimedOut,
                Err(_) => {
                    WorkerOutcome::Failed("Worker task panicked; owned work was cleaned up".into())
                }
            };
            let mut report = execution.report(outcome, started);
            let persisted = parent_session
                .as_ref()
                .map(|session| {
                    session.record(crate::session::Event::Worker(Box::new(WorkerView {
                        worker_id: execution.id.clone(),
                        request: execution.request.clone(),
                        status: report.status,
                        report: Some(report.clone()),
                    })))
                })
                .transpose();
            if let Err(error) = persisted {
                report.status = WorkerStatus::Failed;
                report
                    .unresolved_issues
                    .push(format!("Worker report persistence failed: {error}"));
            }
            drop(prepared.writer);
            registry.complete(&owner, report);
        });
        Ok(initial)
    }
}

impl WorkerExecution {
    pub(super) fn report(&self, outcome: WorkerOutcome, started: Instant) -> WorkerReport {
        let evidence = self.evidence.lock().unwrap();
        let budget = self.budget.usage();
        let status = match budget.state {
            BudgetState::Available => outcome.status(),
            BudgetState::Exhausted => WorkerStatus::BudgetExhausted,
        };
        let mut unresolved = evidence.unresolved.clone();
        let findings = outcome.findings();
        let findings = match findings.len() <= 8192 {
            true => findings,
            false => {
                unresolved.push("Model explanation was truncated to 8 KiB; the full response remains in the worker session".into());
                findings[..findings.floor_char_boundary(8192)].to_owned()
            }
        };
        if self.request.role == WorkerRole::Write && evidence.validation.is_empty() {
            unresolved.push("No validation checks were executed by this worker; the parent must assess the completion criteria and validate changes".into());
        }
        let snapshot = self
            .session
            .lock()
            .unwrap()
            .as_ref()
            .map(|session| session.snapshot())
            .transpose();
        let snapshot = match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                unresolved.push(format!(
                    "Could not retrieve worker session evidence: {error}"
                ));
                None
            }
        };
        let artifacts = snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .artifacts
                    .iter()
                    .filter(|artifact| !evidence.inherited_artifacts.contains(&artifact.id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let processes = snapshot
            .map(|snapshot| snapshot.processes.into_values().collect())
            .unwrap_or_default();
        WorkerReport {
            worker_id: self.id.clone(),
            status,
            findings,
            changed_files: evidence.changed_files.iter().cloned().collect(),
            possibly_changed_files: evidence.possibly_changed_files.iter().cloned().collect(),
            validation: evidence.validation.clone(),
            edits: evidence.edits.clone(),
            processes,
            unresolved_issues: unresolved,
            artifacts,
            budget,
            duration_ms: started.elapsed().as_millis(),
            completion_criteria: self.request.completion_criteria.clone(),
        }
    }
}
