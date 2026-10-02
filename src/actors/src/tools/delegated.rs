use crate::actor::{ActorContext, ActorInfo};
use crate::states::runtime::ExecutionRole;
use analysis::contexts::rust_context::RustContext;
use worker_registry::report::WorkerReport;
use worker_registry::request::WorkerRequest;
use worker_registry::workflow::{BuiltinAgent, StepOutput, Workflow, WorkflowReport};

pub(super) fn execution_budget() -> std::time::Duration {
    std::time::Duration::from_secs(worker_registry::request::BudgetLimits::DEFAULT_SECONDS)
}

pub(super) async fn run(
    objective: String,
    agent: BuiltinAgent,
    context: &RustContext,
    actor: &ActorContext<RustContext>,
) -> anyhow::Result<WorkerReport> {
    let info = super::start_worker::info(actor)?;
    let workflow = Workflow::single(agent, objective, |name| {
        info.services.tool(name).map(|tool| tool.effect())
    })?;
    let result = run_workflow(workflow, context, actor).await?;
    match result.steps.into_iter().next().map(|step| step.output) {
        Some(StepOutput::Agent { report }) => Ok(*report),
        Some(StepOutput::Failed { message }) => Err(anyhow::anyhow!(message)),
        _ => Err(anyhow::anyhow!("Workflow stopped without a worker report")),
    }
}

pub(super) async fn run_workflow(
    workflow: Workflow,
    context: &RustContext,
    actor: &ActorContext<RustContext>,
) -> anyhow::Result<WorkflowReport> {
    let info = super::start_worker::info(actor)?;
    match info.runtime.role {
        ExecutionRole::Root => Ok(()),
        _ => Err(anyhow::anyhow!("Only the root can run workflows")),
    }?;
    info.runtime.interaction.authorize(workflow.effect())?;
    Ok(workflow
        .run(|request| run_request(request, context, info))
        .await)
}

async fn run_request(
    request: WorkerRequest,
    context: &RustContext,
    info: &ActorInfo<RustContext>,
) -> anyhow::Result<WorkerReport> {
    let registry = &info.runtime.workers;
    let owner = info.owner.clone();
    let mut view = crate::worker_registry::launch::start(registry, info, context, request)?;
    while !view.status.terminal() {
        view = registry.wait(&owner, &view.worker_id, 60).await?;
    }
    view.report
        .ok_or_else(|| anyhow::anyhow!("Worker stopped without a report"))
}
