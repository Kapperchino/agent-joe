use analysis::contexts::context::Context;
use clients::failure::{Failure, FailureKind};
use clients::llm::LLmClient;
use common_models::tui_models::ActorToTui;
use flume::Sender;
use std::panic::AssertUnwindSafe;
use tools::tool_defs::{ErasedToolRef, ToolDefinition, ToolResult};

pub struct ActorServices<C: Context, A> {
    pub client: LLmClient,
    pub tools: Vec<ErasedToolRef<C, A>>,
    pub tui_tx: Sender<ActorToTui>,
    pub debug_mode: bool,
}

impl<C: Context, A> ActorServices<C, A> {
    pub fn tool(&self, name: &str) -> Option<&ErasedToolRef<C, A>> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|tool| tool.definition()).collect()
    }

    pub fn update_context(&self, context: &mut C, result: &ToolResult) -> Result<(), Failure> {
        match (&result.outcome, self.tool(result.invocation.name.as_ref())) {
            (Ok(content), Some(tool)) => {
                let input = serde_json::Value::Object(result.invocation.input.clone());
                match std::panic::catch_unwind(AssertUnwindSafe(|| {
                    tool.add_context(&input, context, content)
                })) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(Failure::new(
                        FailureKind::Tool,
                        format!("Tool completed but context update failed: {error}"),
                    )),
                    Err(_) => Err(Failure::new(
                        FailureKind::Tool,
                        "Tool completed but context hook panicked",
                    )),
                }
            }
            _ => Ok(()),
        }
    }
}
