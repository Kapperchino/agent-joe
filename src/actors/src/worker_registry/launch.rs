use super::WorkerSession;
use crate::actor::{ActorInfo, Dependency};
use crate::states::runtime::ExecutionRole;
use crate::worker::{ContextWorker, run_worker};
use crate::workers::task_worker::TaskWorker;
use analysis::contexts::{context::Context, rust_context::RustContext};
use futures::FutureExt;
use std::{
    panic::AssertUnwindSafe,
    sync::Arc,
    time::{Duration, Instant},
};
use turn_engine::WorkerFailure;
use utils::execution::ExecutionScope;
use worker_registry::WorkerRegistry;
use worker_registry::report::{WorkerOutcome, WorkerStatus, WorkerView};
use worker_registry::request::{WorkerRequest, WorkerRole};

struct PreparedWorker {
    parent_scope: ExecutionScope,
    scope: ExecutionScope,
    dependency: Dependency<RustContext>,
    writer: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl PreparedWorker {
    fn new(
        info: &ActorInfo<RustContext>,
        context: &RustContext,
        request: &WorkerRequest,
    ) -> anyhow::Result<Self> {
        let effect = match request.role() {
            WorkerRole::Read => tools::tool_defs::ToolEffect::DelegateRead,
            WorkerRole::Write => tools::tool_defs::ToolEffect::DelegateWrite,
        };
        info.runtime.interaction.authorize(effect)?;
        let parent_scope = match &info.runtime.role {
            ExecutionRole::Root => info
                .runtime
                .turn_scope
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Workers require an active parent turn")),
            ExecutionRole::Worker { .. } => Err(anyhow::anyhow!("Maximum worker depth is one")),
            ExecutionRole::Helper => Err(anyhow::anyhow!("Only the root can start workers")),
        }?;
        let access = match request.role() {
            WorkerRole::Read => utils::workspace::RootAccess::ReadOnly,
            WorkerRole::Write => utils::workspace::RootAccess::ReadWrite,
        };
        let scope = parent_scope.restricted_child(request.allowed_paths(), access)?;
        let workspace = scope.workspace()?;
        let tools = request
            .allowed_tools()
            .iter()
            .map(|name| {
                let tool = info.services.tool(name)
                    .filter(|tool| !tool.effect().delegates())
                    .filter(|_| !matches!(name.as_str(), "update_plan" | "request_user_input"))
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
                        "Tool `{name}` requires allowed_paths: .; repository inspection, worktree management and Cargo require whole-project paths. For narrower paths use find_files, list_directory, grep and read_file; ask the root to review and validate"
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
            task_prompt: Some(request.prompt(&info.runtime.inherited_constraints)?),
            guidance: context.guidance.fork(),
            id_gen: Arc::new(std::sync::atomic::AtomicU64::new(context.gen_id())),
            ..context.clone()
        };
        let writer = match request.role() {
            WorkerRole::Write => Some(info.runtime.workspace.writer.clone().try_lock_owned().map_err(|_| anyhow::anyhow!("A writer already owns the workspace; wait before starting another write worker"))?),
            WorkerRole::Read => None,
        };
        let runtime = crate::states::runtime::Runtime {
            turn_scope: None,
            ..info.runtime.child(scope.clone())
        };
        Ok(Self {
            parent_scope,
            scope,
            dependency: Dependency {
                client: info.services.client.clone(),
                tui_tx: info.services.tui_tx.clone(),
                debug_mode: info.services.debug_mode,
                context,
                tools,
                runtime,
            },
            writer,
        })
    }
}

pub(crate) fn start(
    registry: &Arc<WorkerRegistry>,
    info: &ActorInfo<RustContext>,
    context: &RustContext,
    request: WorkerRequest,
) -> anyhow::Result<WorkerView> {
    let mut prepared = PreparedWorker::new(info, context, &request)?;
    let owner = info.owner.clone();
    let execution = registry.register(&owner, prepared.scope.cancel.clone(), request)?;
    let session = Arc::new(WorkerSession::default());
    let initial = WorkerView {
        worker_id: execution.id.clone(),
        request: execution.request.clone(),
        status: WorkerStatus::Registered,
        report: None,
    };
    let parent_session = info.runtime.session.clone();
    parent_session
        .as_ref()
        .map(|session| session.record(crate::session::Event::Worker(Box::new(initial.clone()))))
        .transpose()
        .map_err(|error| {
            registry.complete(
                &owner,
                execution.report(
                    WorkerOutcome::Failed(format!(
                        "Worker registration persistence failed: {error}"
                    )),
                    Duration::ZERO,
                    Default::default(),
                ),
            );
            error
        })?;
    prepared.dependency.runtime.role = ExecutionRole::Worker {
        execution: execution.clone(),
        session: session.clone(),
    };
    let parent = info.actor_ref.clone();
    let registry = registry.clone();
    prepared.parent_scope.tasks.clone().spawn(async move {
        let runner = prepared.parent_scope.child();
        let started = Instant::now();
        registry.running(&owner, &execution.id);
        let result = AssertUnwindSafe(async {
            tokio::time::timeout(
                Duration::from_secs(execution.request.budget().seconds()),
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
        let mut report = execution.report(outcome, started.elapsed(), session.evidence());
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
