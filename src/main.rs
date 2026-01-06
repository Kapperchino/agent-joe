mod actor;
mod actor_state;
mod analysis;
mod app;
mod cache;
mod claude;
mod cur_context;
mod tool_defs;
mod tool_impls;
mod utils;
mod worker;

use crate::actor::Dependency;
use crate::claude::{ClaudeClient, ClaudeConfig};
use crate::tool_impls::ReadFile;
use crate::worker::Worker;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use log::LevelFilter;
use ractor::Actor;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::env;
use std::io::{stdout, Write};
use tokio::main;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

#[main]
async fn main() {
    CombinedLogger::init(vec![TermLogger::new(
        LevelFilter::Info,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )])
    .unwrap();

    let client = ClaudeClient::new(ClaudeConfig {
        api_key: env::var("CLAUDE_API").unwrap(),
        ..Default::default()
    })
    .unwrap();

    let (tx, rx) = mpsc::unbounded_channel();

    let (joe, actor_handle) = Actor::spawn(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![tool_impls::Tool::ReadFile(ReadFile::default())],
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
