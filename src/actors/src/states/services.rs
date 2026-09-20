use crate::actor::ActorContext;
use crate::states::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use common_models::tui_models::ActorToTui;
use flume::Sender;
use std::path::PathBuf;
use tools::tool_defs::{ErasedToolRef, ToolDefinition};

pub struct ActorServices<C: Context> {
    pub client: LLmClient,
    pub tools: Vec<ErasedToolRef<C, ActorContext<C>>>,
    pub tui_tx: Sender<ActorToTui>,
    pub debug_mode: bool,
}

impl<C: Context> ActorServices<C> {
    pub fn tool(&self, name: &str) -> Option<&ErasedToolRef<C, ActorContext<C>>> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|tool| tool.definition()).collect()
    }

    pub fn stream_log(
        &self,
        runtime: &crate::states::runtime::Runtime,
    ) -> anyhow::Result<Option<tokio::fs::File>> {
        match runtime.role {
            ExecutionRole::Root if self.debug_mode => {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let path = PathBuf::from(format!("./logs/stream_{timestamp}.jsonl"));
                let workspace = runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| runtime.scope.workspace())?;
                let file = workspace.open_append(&path)?;
                Ok(Some(tokio::fs::File::from_std(file)))
            }
            _ => Ok(None),
        }
    }
}
