use analysis::contexts::context::Context;
use clients::llm::LLmClient;
use common_models::tui_models::ActorToTui;
use flume::Sender;
use tools::tool_defs::{ErasedToolRef, ToolDefinition};

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
}
