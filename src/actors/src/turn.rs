use crate::{
    runtime::WorkspaceRevision,
    stream_processor::{ProcessedItem, StreamProcessor},
    tool_call::ToolCall,
};
use clients::{
    failure::{Failure, FailureKind},
    llm::{ContentBlock, Message, Role},
};
use common_models::{
    runtime_ids::{OperationId, TurnId},
    tui_models::Lifecycle,
};
use std::collections::HashMap;
use tools::{
    tool_defs::ToolResult,
    tool_error::{ToolFailure, ToolFailureKind},
};
use utils::execution::{ExecutionScope, OwnedScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag {
    pub turn: TurnId,
    pub operation: OperationId,
}
impl Tag {
    pub fn new(turn: TurnId) -> Self {
        Self {
            turn,
            operation: OperationId::new(),
        }
    }
}

pub struct FollowUp {
    pub id: TurnId,
    pub prompt: Option<String>,
}
impl FollowUp {
    pub fn new(prompt: Option<String>) -> Self {
        Self {
            id: TurnId::new(),
            prompt,
        }
    }
}

#[derive(Default)]
pub enum TurnState {
    #[default]
    Idle,
    Provider(Turn<ProviderRun>),
    Tools(Turn<ToolBatch>),
    Stopping(Turn<Cleanup>),
}
impl TurnState {
    pub fn tools_mut(&mut self, tag: Tag) -> Option<(&mut FailureTracker, &mut ToolBatch)> {
        let tools = match self {
            Self::Tools(turn) => Some((&mut turn.failures, &mut turn.phase)),
            Self::Stopping(turn) => match &mut turn.phase.work {
                CleanupWork::Tools(batch) => Some((&mut turn.failures, batch)),
                CleanupWork::Provider(_) => None,
            },
            _ => None,
        };
        tools.filter(|(_, batch)| batch.tag == tag)
    }

    #[cfg(test)]
    pub fn batch(&self) -> Option<&ToolBatch> {
        match self {
            Self::Tools(turn) => Some(&turn.phase),
            Self::Stopping(turn) => match &turn.phase.work {
                CleanupWork::Tools(batch) => Some(batch),
                CleanupWork::Provider(_) => None,
            },
            _ => None,
        }
    }
}

pub struct Turn<P> {
    pub id: TurnId,
    pub scope: OwnedScope,
    pub failures: FailureTracker,
    pub phase: P,
}
impl Turn<ProviderRun> {
    pub fn new(id: TurnId, scope: ExecutionScope) -> Self {
        let phase = ProviderRun::new(id, scope.child(), 0);
        Self {
            id,
            scope: OwnedScope::new(scope),
            failures: FailureTracker::default(),
            phase,
        }
    }
}
impl<P> Turn<P> {
    pub fn map<Q>(self, phase: impl FnOnce(P) -> Q) -> Turn<Q> {
        Turn {
            id: self.id,
            scope: self.scope,
            failures: self.failures,
            phase: phase(self.phase),
        }
    }

    pub fn provider(self) -> Turn<ProviderRun> {
        let run = ProviderRun::new(self.id, self.scope.child(), 0);
        self.map(|_| run)
    }
}
impl<P: Into<CleanupWork>> Turn<P> {
    pub fn stopping(self, outcome: TurnOutcome, history: HistoryDisposition) -> Turn<Cleanup> {
        self.map(|work| Cleanup {
            work: work.into(),
            outcome,
            history,
        })
    }
}

pub struct Cleanup {
    pub work: CleanupWork,
    pub outcome: TurnOutcome,
    pub history: HistoryDisposition,
}
pub enum CleanupWork {
    Provider(Tag),
    Tools(ToolBatch),
}
impl CleanupWork {
    pub fn tag(&self) -> Tag {
        match self {
            Self::Provider(tag) => *tag,
            Self::Tools(batch) => batch.tag,
        }
    }
}
impl From<ProviderRun> for CleanupWork {
    fn from(run: ProviderRun) -> Self {
        Self::Provider(run.tag)
    }
}
impl From<ToolBatch> for CleanupWork {
    fn from(batch: ToolBatch) -> Self {
        Self::Tools(batch)
    }
}

pub struct ProviderRun {
    pub tag: Tag,
    pub scope: ExecutionScope,
    pub attempt: u8,
    pub response: ResponseState,
}
impl ProviderRun {
    pub fn new(turn: TurnId, scope: ExecutionScope, attempt: u8) -> Self {
        Self {
            tag: Tag::new(turn),
            scope,
            attempt,
            response: ResponseState::Streaming,
        }
    }
}
pub enum ResponseState {
    Streaming,
    Complete,
    ToolUse,
}
pub enum AcceptedResponse {
    Complete(Message),
    Tools(ToolBatch),
}
impl ProviderRun {
    pub fn finish(&self, stream: &mut StreamProcessor) -> Result<AcceptedResponse, Failure> {
        match self.response {
            ResponseState::Streaming => Err(Failure::new(
                FailureKind::Transport,
                "Provider stream ended without a complete response",
            )),
            ResponseState::ToolUse => stream
                .extract_and_pre_process()
                .map(|items| AcceptedResponse::Tools(ToolBatch::new(self.tag.turn, items)))
                .map_err(|error| Failure::new(FailureKind::InvalidInput, error.to_string())),
            ResponseState::Complete => stream
                .extract_and_pre_process()
                .and_then(|items| {
                    items
                        .into_iter()
                        .map(|item| match item {
                            ProcessedItem::Content(content) => Ok(content),
                            ProcessedItem::Tool(_) => {
                                Err(anyhow::anyhow!("Tool in a completed response"))
                            }
                        })
                        .collect()
                })
                .map(|content| {
                    AcceptedResponse::Complete(Message {
                        role: Role::Assistant,
                        content,
                    })
                })
                .map_err(|error| Failure::new(FailureKind::InvalidInput, error.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed(Failure),
}
impl TurnOutcome {
    pub fn lifecycle(&self) -> Lifecycle {
        match self {
            Self::Completed => Lifecycle::Completed,
            Self::Cancelled => Lifecycle::Cancelled,
            Self::Failed(_) => Lifecycle::Failed,
        }
    }
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Completed => None,
            Self::Cancelled => Some("Turn cancelled".into()),
            Self::Failed(failure) => Some(failure.to_string()),
        }
    }
}
#[derive(Clone, Copy)]
pub enum HistoryDisposition {
    Retain,
    Clear,
}

#[derive(Clone)]
pub struct ToolJob {
    pub operation: OperationId,
    pub call: ToolCall,
}
pub struct ToolBatch {
    pub tag: Tag,
    assistant: Vec<ContentBlock>,
    entries: Vec<ToolEntry>,
    pub continuation: Result<(), Failure>,
}
struct ToolEntry {
    job: ToolJob,
    state: ToolState,
}
enum ToolState {
    Queued,
    Running,
    Completed(ToolResult),
}

impl ToolBatch {
    pub fn new(turn: TurnId, items: Vec<ProcessedItem>) -> Self {
        let mut assistant = Vec::new();
        let mut entries = Vec::new();
        for item in items {
            match item {
                ProcessedItem::Content(content) => assistant.push(content),
                ProcessedItem::Tool(call) => {
                    assistant.push(call.content());
                    entries.push(ToolEntry {
                        job: ToolJob {
                            operation: OperationId::new(),
                            call,
                        },
                        state: ToolState::Queued,
                    });
                }
            }
        }
        Self {
            tag: Tag::new(turn),
            assistant,
            entries,
            continuation: Ok(()),
        }
    }

    pub fn jobs(&self) -> Vec<ToolJob> {
        self.entries.iter().map(|entry| entry.job.clone()).collect()
    }

    pub fn start(&mut self, operation: OperationId) -> Option<&ToolJob> {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.job.operation == operation && matches!(entry.state, ToolState::Queued)
            })
            .map(|entry| {
                entry.state = ToolState::Running;
                &entry.job
            })
    }

    pub fn complete(&mut self, operation: OperationId, result: ToolResult) -> Option<ToolResult> {
        self.entries
            .iter_mut()
            .find(|entry| {
                entry.job.operation == operation
                    && !matches!(entry.state, ToolState::Completed(_))
                    && result.id == entry.job.call.id
                    && result.invocation.name == entry.job.call.name
            })
            .map(|entry| {
                entry.state = ToolState::Completed(result.clone());
                result
            })
    }

    pub fn pending_operations(&self) -> impl Iterator<Item = OperationId> + '_ {
        self.entries
            .iter()
            .filter(|entry| !matches!(entry.state, ToolState::Completed(_)))
            .map(|entry| entry.job.operation)
    }

    pub fn messages(&self) -> [Message; 2] {
        let outputs = self.entries.iter().map(|entry| match &entry.state {
            ToolState::Completed(result) => ContentBlock::ToolResult {
                tool_id: result.id.clone(),
                content: result.outcome.clone().unwrap_or_else(|failure| failure.to_string()),
                is_error: result.outcome.is_err().then_some(true),
            },
            ToolState::Queued => entry.job.call.error_content("Not executed: the turn stopped before this tool started."),
            ToolState::Running => entry.job.call.error_content("Interrupted operation: completion is unknown. Inspect possible partial effects before retrying."),
        }).collect();
        [
            Message {
                role: Role::Assistant,
                content: self.assistant.clone(),
            },
            Message {
                role: Role::User,
                content: outputs,
            },
        ]
    }
}

#[derive(Default)]
pub struct FailureTracker(HashMap<FailureFingerprint, u8>);
#[derive(PartialEq, Eq, Hash)]
struct FailureFingerprint {
    revision: WorkspaceRevision,
    tool: String,
    arguments: String,
    kind: ToolFailureKind,
    message: String,
}
impl FailureTracker {
    pub fn record(
        &mut self,
        result: &ToolResult,
        revision: WorkspaceRevision,
    ) -> Result<(), Failure> {
        match &result.outcome {
            Err(failure) => {
                let fingerprint = FailureFingerprint {
                    revision,
                    tool: result.invocation.name.to_string(),
                    arguments: serde_json::Value::Object(result.invocation.input.clone())
                        .to_string(),
                    kind: failure.kind,
                    message: failure.message.clone(),
                };
                let count = self.0.entry(fingerprint).or_default();
                *count = count.saturating_add(1);
                if *count >= 3 || failure.stops_turn() {
                    Err(tool_failure(failure))
                } else {
                    Ok(())
                }
            }
            Ok(_) => Ok(()),
        }
    }
}

pub fn tool_failure(failure: &ToolFailure) -> Failure {
    let kind = match failure.kind {
        ToolFailureKind::Worker => FailureKind::Worker,
        ToolFailureKind::InvalidInput => FailureKind::InvalidInput,
        _ => FailureKind::Tool,
    };
    Failure::new(
        kind,
        format!("{failure}. Automatic continuation stopped; inspect the result before retrying."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Workspace;
    use tools::{
        tool_defs::{ToolEffect, ToolId},
        tool_error::ToolEffects,
    };

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: ToolId {
                id: id.to_owned().try_into().unwrap(),
                call_id: None,
            },
            name: "read".to_owned().try_into().unwrap(),
            input: Default::default(),
        }
    }

    fn failure(call: &ToolCall) -> ToolResult {
        call.failed(ToolFailure::new(
            ToolFailureKind::Execution,
            ToolEffects::NoWorkspaceChange,
            "file unavailable",
        ))
    }

    #[test]
    fn batch_rejects_unknown_mismatched_and_duplicate_completions() {
        let call = call("accepted");
        let mut batch = ToolBatch::new(TurnId::new(), vec![ProcessedItem::Tool(call.clone())]);
        let operation = batch.jobs()[0].operation;
        assert!(batch.complete(OperationId::new(), failure(&call)).is_none());
        let mut other = call.clone();
        other.id.id = "other".to_owned().try_into().unwrap();
        assert!(batch.complete(operation, failure(&other)).is_none());
        assert!(batch.start(operation).is_some());
        assert!(batch.start(operation).is_none());
        assert!(batch.complete(operation, failure(&call)).is_some());
        assert!(batch.complete(operation, failure(&call)).is_none());
        assert!(batch.start(operation).is_none());
        assert_eq!(batch.pending_operations().count(), 0);
        assert!(
            matches!(&batch.messages()[1].content[0], ContentBlock::ToolResult {
            tool_id, content, is_error: Some(true),
        } if *tool_id == call.id && content == "Execution: file unavailable")
        );
    }

    #[tokio::test]
    async fn workspace_mutation_resets_the_repeated_failure_budget() {
        let workspace = Workspace::new(4);
        let mut failures = FailureTracker::default();
        let result = failure(&call("accepted"));
        for _ in 0..2 {
            assert!(matches!(
                failures.record(&result, workspace.revision()),
                Ok(())
            ));
        }
        drop(
            workspace
                .acquire(ToolEffect::Write, &ExecutionScope::default())
                .await
                .unwrap(),
        );
        for _ in 0..2 {
            assert!(matches!(
                failures.record(&result, workspace.revision()),
                Ok(())
            ));
        }
        assert!(matches!(
            failures.record(&result, workspace.revision()),
            Err(_)
        ));
    }
}
