use crate::{
    actor::ActorContext,
    worker_registry::{
        report::WorkerReport,
        request::{WorkerRequest, WorkerRequestInput},
    },
};
use analysis::contexts::rust_context::RustContext;

pub(super) async fn run(
    objective: String,
    tools: &str,
    context: &RustContext,
    actor: &ActorContext<RustContext>,
) -> anyhow::Result<WorkerReport> {
    let info = super::start_worker::info(actor)?;
    let request = WorkerRequest::new(WorkerRequestInput {
        objective,
        constraints: "Preserve all inherited constraints and unrelated user changes".into(),
        allowed_tools: tools.into(),
        allowed_paths: ".".into(),
        context: String::new(),
        completion_criteria: "Complete the stated objective; report findings, requested versus executed validation, and every remaining limitation".into(),
        ..Default::default()
    }, |name| info.services.tool(name).map(|tool| tool.effect()))?;
    let registry = &info.runtime.workers;
    let owner = info.owner.clone();
    let mut view = registry.start(info, context, request)?;
    while !view.status.terminal() {
        view = registry.wait(&owner, &view.worker_id, 60).await?;
    }
    view.report
        .ok_or_else(|| anyhow::anyhow!("Worker stopped without a report"))
}
