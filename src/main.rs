mod actor;
mod actor_state;
mod analysis;
mod app;
mod cache;
mod cache_actor;
mod claude;
mod cur_context;
mod file_actor;
mod proj_meta;
mod rust_proj;
mod symbol_info;
mod text_search;
mod tool_defs;
mod tool_impls;
mod utils;
mod worker;
mod cargo;

use crate::actor::Dependency;
use crate::claude::{ClaudeClient, ClaudeConfig};
use crate::tool_defs::{CargoCheck, InsertAfterLine, StringReplace};
use crate::tool_impls::ReadFile;
use crate::worker::Worker;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use ractor::Actor;
use std::env;
use std::io::{stdout, Write};
use tokio::main;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[main]
async fn main() {
    let file = std::fs::File::create("err.log").unwrap();
    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::WARN)
        .with_writer(file)
        // completes the builder.
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let client = ClaudeClient::new(ClaudeConfig {
        api_key: env::var("CLAUDE_API").unwrap(),
        model: "claude-haiku-4-5-20251001".to_string(),
        ..Default::default()
    })
    .unwrap();

    let (tx, rx) = mpsc::unbounded_channel();

    let (joe, actor_handle) = Actor::spawn(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![
                tool_impls::Tool::ReadFile(ReadFile::default()),
                tool_impls::Tool::InsertAfterLine(InsertAfterLine::default()),
                tool_impls::Tool::StringReplace(StringReplace::default()),
                tool_impls::Tool::CargoCheck(CargoCheck::default())
            ],
            tui_tx: tx,
        },
    )
    .await
    .expect("Failed to start actor");

    color_eyre::install().unwrap();
    let terminal = ratatui::init();
    execute!(stdout(), EnableBracketedPaste).ok();
    let app = crate::app::App::new(joe);
    let app_result = app.run(terminal, rx).await.unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
    execute!(stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
}
