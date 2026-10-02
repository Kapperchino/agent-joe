use crate::WorkflowInput;
use clients::llm::{ContentBlock, Message};
use clients::response::ToolCall;
use common_models::runtime_ids::TurnId;
use tools::tool_defs::{NonEmptyString, ToolId};
use turn_engine::turn::{FollowUp, TurnStart};

fn call_id(turn: TurnId) -> String {
    format!("fc_workflow_{turn}")
}

impl WorkflowInput {
    pub fn follow_up(self, turn: TurnId, prompt: Option<String>) -> anyhow::Result<FollowUp> {
        let input = serde_json::from_value(serde_json::to_value(self)?)?;
        let id = NonEmptyString::try_from(call_id(turn))?;
        Ok(FollowUp {
            id: turn,
            prompt,
            start: TurnStart::Tool {
                call: Box::new(ToolCall {
                    id: ToolId {
                        id: id.clone(),
                        call_id: Some(id),
                    },
                    name: "run_workflow".to_string().try_into()?,
                    input,
                }),
            },
        })
    }
}

pub struct WorkflowCompletion;

#[derive(Default)]
enum CompletionState {
    #[default]
    Pending,
    Completed,
    Failed,
    Ambiguous,
}

impl WorkflowCompletion {
    pub fn new(turn: TurnId, history: &[Message]) -> anyhow::Result<Self> {
        let id = call_id(turn);
        let outcome = history
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_id, is_error, ..
                } if tool_id.id.as_ref() == id => Some(*is_error),
                _ => None,
            })
            .fold(CompletionState::Pending, |state, result| {
                match (state, result) {
                    (CompletionState::Pending, None | Some(false)) => CompletionState::Completed,
                    (CompletionState::Pending, Some(true)) => CompletionState::Failed,
                    _ => CompletionState::Ambiguous,
                }
            });
        match outcome {
            CompletionState::Completed => Ok(Self),
            CompletionState::Failed => Err(anyhow::anyhow!(
                "The workflow stopped or failed; automatic merge cannot continue"
            )),
            CompletionState::Pending => Err(anyhow::anyhow!(
                "The workflow has not completed; automatic merge cannot continue"
            )),
            CompletionState::Ambiguous => Err(anyhow::anyhow!(
                "The workflow has repeated results; automatic merge cannot continue"
            )),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/continuation.rs"]
mod tests;
