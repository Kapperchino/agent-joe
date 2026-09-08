use crate::actor::{ActorContext, Dependency, IntoActorErr, Message};
use crate::actor_state::ActorState;
use crate::worker::Worker;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContext;
use async_trait::async_trait;
use ractor::{ActorProcessingErr, ActorRef};
use std::marker::PhantomData;
use tools::apply_patch::ApplyPatch;
use tools::cargo_check::CargoCheck;
use tools::cargo_test::CargoTest;
use tools::grep::GrepTool;
use tools::read_file::ReadFile;
use tools::tool_defs::{ErasedToolRef, erased_tool};
use tools::web_search::WebSearch;

pub struct SimpleWorker<C: Context> {
    _ctx: PhantomData<C>,
}

const PROMPT: &str = include_str!("resources/simple_worker.md");

#[async_trait]
impl Worker for SimpleWorker<RustContext> {
    type C = RustContext;

    fn init_prompt(added: Option<&str>) -> String {
        let question = added.unwrap_or_default();
        format!("{PROMPT}\n\n{question}")
    }

    async fn startup_hook(
        &self,
        myself: ActorRef<Message>,
        dependency: Dependency<Self::C>,
    ) -> Result<ActorState<Self::C>, ActorProcessingErr> {
        let file_actor =
            crate::background_actors::file_actor::start(&dependency.context, &myself).await?;
        let state = ActorState::new(dependency, myself.clone(), Some(file_actor))
            .await
            .actor_err()?;
        Ok(state)
    }

    fn tools() -> Vec<ErasedToolRef<Self::C, ActorContext<Self::C>>> {
        vec![
            erased_tool::<tools::find_files::FindFiles, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::list_directory::ListDirectory, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::inspect_context::InspectContext, Self::C, ActorContext<Self::C>>(),
            erased_tool::<ReadFile, Self::C, ActorContext<Self::C>>(),
            erased_tool::<GrepTool, Self::C, ActorContext<Self::C>>(),
            erased_tool::<ApplyPatch, Self::C, ActorContext<Self::C>>(),
            erased_tool::<CargoCheck, Self::C, ActorContext<Self::C>>(),
            erased_tool::<CargoTest, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::CargoFmt, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::CargoFmtCheck, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::CargoClippy, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::CargoRun, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::CargoStart, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::ProcessPoll, Self::C, ActorContext<Self::C>>(),
            erased_tool::<tools::cargo_tools::ProcessStop, Self::C, ActorContext<Self::C>>(),
            erased_tool::<WebSearch, Self::C, ActorContext<Self::C>>(),
        ]
    }
}

impl<C: Context> SimpleWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
