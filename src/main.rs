mod actor;
mod actor_state;
mod app;
mod claude;
mod cur_context;
mod tools;
mod worker;

use crate::claude::{ClaudeClient, ClaudeConfig};
use log::LevelFilter;
use ractor::Actor;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::env;
use std::io::Write;
use tokio::main;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use crate::actor::Dependency;
use crate::tools::ReadFile;
use crate::worker::Worker;

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

    let (tx, rx) = mpsc::channel(32);

    let (joe, actor_handle) = Actor::spawn(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![tools::Tool::ReadFile(ReadFile::default())],
            tui_tx: tx,
        },
    )
    .await
    .expect("Failed to start actor");

    color_eyre::install().unwrap();
    let terminal = ratatui::init();
    let app = crate::app::App::new(joe);
    let app_result = app.run(terminal, rx).await.unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
    ratatui::restore();
}
