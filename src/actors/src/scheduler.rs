use crate::{
    actor::{ActorContext, ActorInfo, Dependency, Message},
    runtime::WorkspaceRevision,
    turn::{Tag, ToolJob},
};
use analysis::contexts::context::Context;
use common_models::runtime_ids::OperationId;
use futures::{FutureExt, StreamExt};
use ractor::ActorRef;
use std::{collections::VecDeque, panic::AssertUnwindSafe};
use tools::{
    tool_defs::{ErasedToolRef, ToolEffect, ToolInvocation, ToolResult},
    tool_error::{ToolEffects, ToolFailure, ToolFailureKind},
};
use utils::execution::{ExecutionScope, ResourceKind};

#[derive(Debug)]
pub enum ToolEvent {
    Started {
        operation: OperationId,
        effect: ToolEffect,
        revision: Option<WorkspaceRevision>,
        display: String,
    },
    Completed {
        operation: OperationId,
        result: ToolResult,
    },
    Finished(Result<(), ToolFailure>),
}

#[derive(Clone)]
pub struct Executor<C: Context> {
    pub dependency: Dependency<C>,
    pub context: C,
    pub actor: ActorRef<Message>,
}

struct PreparedTool<C: Context> {
    implementation: ErasedToolRef<C, ActorContext<C>>,
    job: ToolJob,
    display: String,
    effect: ToolEffect,
}

enum ToolGroup {
    Reads(Vec<ToolJob>),
    Exclusive(ToolJob),
}

impl<C: Context + Clone + 'static> PreparedTool<C> {
    fn new(
        job: ToolJob,
        implementation: ErasedToolRef<C, ActorContext<C>>,
    ) -> Result<Self, ToolFailure> {
        implementation
            .display_erased(&job.call.input_value())
            .map_err(|error| {
                ToolFailure::new(
                    ToolFailureKind::InvalidInput,
                    ToolEffects::NotStarted,
                    error.to_string(),
                )
            })
            .map(|display| Self {
                effect: implementation.effect(),
                implementation,
                job,
                display,
            })
    }

    fn into_result(self, outcome: Result<String, ToolFailure>) -> ToolResult {
        ToolResult {
            id: self.job.call.id,
            invocation: ToolInvocation {
                name: self.job.call.name,
                input: self.job.call.input,
                display: self.display,
            },
            outcome,
        }
    }

    fn content(
        &self,
        input: &serde_json::Value,
        output: &serde_json::Value,
    ) -> Result<String, ToolFailure> {
        match self.implementation.output_to_content_erased(input, output) {
            Ok(content) => match self.implementation.output_is_error_erased(output) {
                Ok(false) => Ok(content),
                Ok(true) => Err(ToolFailure::new(
                    ToolFailureKind::Validation,
                    ToolEffects::NoWorkspaceChange,
                    content,
                )),
                Err(error) => Err(execution_error(error, self.effect)),
            },
            Err(error) => Err(execution_error(error, self.effect)),
        }
    }
}

impl<C: Context + Clone + 'static> Executor<C> {
    fn prepare(&self, job: ToolJob) -> Result<PreparedTool<C>, ToolFailure> {
        self.dependency
            .tool(job.call.name.as_ref())
            .cloned()
            .ok_or_else(|| {
                ToolFailure::new(
                    ToolFailureKind::InvalidInput,
                    ToolEffects::NotStarted,
                    format!("unknown tool `{}`", job.call.name),
                )
            })
            .and_then(|tool| PreparedTool::new(job, tool))
    }

    fn emit(&self, tag: Tag, event: ToolEvent) {
        let _ = self.actor.send_message(Message::Tools { tag, event });
    }

    async fn execute(&self, job: ToolJob, tag: Tag) -> ToolResult {
        let prepared = std::panic::catch_unwind(AssertUnwindSafe(|| self.prepare(job.clone())))
            .unwrap_or_else(|_| {
                Err(ToolFailure::new(
                    ToolFailureKind::Panicked,
                    ToolEffects::NotStarted,
                    "Tool preparation panicked",
                ))
            });
        let result = match prepared {
            Ok(prepared) => {
                let scope = self.dependency.runtime.scope.child();
                let _registration =
                    scope.register(ResourceKind::Tool, prepared.job.call.name.to_string());
                let outcome = match self
                    .dependency
                    .runtime
                    .workspace
                    .acquire(prepared.effect, &scope)
                    .await
                {
                    Ok(lease) => {
                        self.emit(
                            tag,
                            ToolEvent::Started {
                                operation: job.operation,
                                effect: prepared.effect,
                                revision: lease.revision(),
                                display: prepared.display.clone(),
                            },
                        );
                        let outcome = AssertUnwindSafe(self.invoke(&prepared, &scope))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| {
                                Err(ToolFailure::new(
                                    ToolFailureKind::Panicked,
                                    effects(prepared.effect),
                                    "Tool implementation panicked",
                                ))
                            });
                        scope.finish().await;
                        drop(lease);
                        outcome
                    }
                    Err(failure) => Err(failure),
                };
                prepared.into_result(outcome)
            }
            Err(failure) => job.call.failed(failure),
        };
        self.emit(
            tag,
            ToolEvent::Completed {
                operation: job.operation,
                result: result.clone(),
            },
        );
        result
    }

    async fn invoke(
        &self,
        prepared: &PreparedTool<C>,
        scope: &ExecutionScope,
    ) -> Result<String, ToolFailure> {
        let runtime = &self.dependency.runtime;
        let input = prepared.job.call.input_value();
        let context = ActorContext::ActorInfo(ActorInfo {
            dep: Dependency {
                runtime: runtime.child(scope.clone()),
                ..self.dependency.clone()
            },
            actor_ref: self.actor.clone(),
        });
        let run = scope.enter(prepared.implementation.run_erased(
            input.clone(),
            prepared.job.call.id.clone(),
            &self.context,
            &context,
        ));
        let output = tokio::select! {
            biased;
            _ = scope.cancel.cancelled() => Err(ToolFailure::new(ToolFailureKind::Cancelled, effects(prepared.effect), "Tool cancelled")),
            result = tokio::time::timeout(runtime.tool_timeout, run) => result
                .map_err(|_| ToolFailure::new(ToolFailureKind::Timeout, effects(prepared.effect), "Tool deadline exceeded"))
                .and_then(|result| result.map_err(|error| execution_error(error, prepared.effect))),
        };
        output.and_then(|output| prepared.content(&input, &output))
    }

    pub fn spawn(self, jobs: Vec<ToolJob>, tag: Tag) {
        self.dependency
            .runtime
            .scope
            .tasks
            .clone()
            .spawn(async move {
                let result = AssertUnwindSafe(self.run(jobs, tag)).catch_unwind().await;
                let result = result.map(|_| ()).map_err(|_| {
                    ToolFailure::new(
                        ToolFailureKind::Panicked,
                        ToolEffects::MayHaveChanged,
                        "Tool scheduler panicked",
                    )
                });
                self.emit(tag, ToolEvent::Finished(result));
            });
    }

    fn concurrent(&self, job: &ToolJob) -> bool {
        self.dependency
            .tool(job.call.name.as_ref())
            .is_some_and(|tool| tool.effect().concurrent())
    }

    fn next_group(&self, pending: &mut VecDeque<ToolJob>) -> Option<ToolGroup> {
        pending.pop_front().map(|job| {
            if self.concurrent(&job) {
                let mut reads = vec![job];
                while pending.front().is_some_and(|job| self.concurrent(job)) {
                    reads.extend(pending.pop_front());
                }
                ToolGroup::Reads(reads)
            } else {
                ToolGroup::Exclusive(job)
            }
        })
    }

    async fn run(&self, jobs: Vec<ToolJob>, tag: Tag) -> Vec<ToolResult> {
        let mut pending = VecDeque::from(jobs);
        let mut results = Vec::new();
        while let Some(group) = self.next_group(&mut pending) {
            if self.dependency.runtime.scope.cancel.is_cancelled() {
                break;
            }
            let completed = match group {
                ToolGroup::Reads(jobs) => {
                    futures::stream::iter(jobs.into_iter().map(|job| self.execute(job, tag)))
                        .buffered(self.dependency.runtime.workspace.read_limit())
                        .collect::<Vec<_>>()
                        .await
                }
                ToolGroup::Exclusive(job) => vec![self.execute(job, tag).await],
            };
            let stop = completed
                .iter()
                .any(|result| result.outcome.as_ref().is_err_and(ToolFailure::stops_turn));
            results.extend(completed);
            if stop {
                break;
            }
        }
        results
    }

    #[cfg(test)]
    pub async fn replay(&self, mut batch: crate::turn::ToolBatch) -> crate::turn::ToolBatch {
        let jobs = batch.jobs();
        let results = self.run(jobs.clone(), batch.tag).await;
        for (job, result) in jobs.into_iter().zip(results) {
            assert!(batch.complete(job.operation, result).is_some());
        }
        batch
    }
}

fn effects(effect: ToolEffect) -> ToolEffects {
    match effect {
        ToolEffect::Write | ToolEffect::DelegateWrite => ToolEffects::MayHaveChanged,
        _ => ToolEffects::NoWorkspaceChange,
    }
}
fn execution_error(error: anyhow::Error, effect: ToolEffect) -> ToolFailure {
    error
        .downcast_ref::<ToolFailure>()
        .cloned()
        .unwrap_or_else(|| {
            ToolFailure::new(
                ToolFailureKind::Execution,
                effects(effect),
                error.to_string(),
            )
        })
}
