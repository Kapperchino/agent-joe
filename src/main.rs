mod actor;
mod actor_state;
mod app;
mod claude;
mod cur_context;
mod tools;
mod utils;
mod worker;
mod analysis;
mod cache;

use crate::actor::Dependency;
use crate::claude::{ClaudeClient, ClaudeConfig};
use crate::tools::{ListFiles, ReadFile};
use crate::worker::Worker;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use heed::EnvOpenOptions;
use log::LevelFilter;
use ractor::Actor;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::env;
use std::io::{stdout, ErrorKind, Write};
use tokio::sync::mpsc;
use tokio::{fs, main};
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

    match fs::create_dir("~/.turbo-code/").await {
        Ok(_) => Ok(()),
        Err(err) => {
            if err.kind() != ErrorKind::AlreadyExists {
                Err(err)
            } else {
                Ok(())
            }
        }
    }
    .unwrap();

    let env = unsafe {
        EnvOpenOptions::new() // 100 MiB
            .open(&"~/.turbo-code/")
    }
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
            tools: vec![
                tools::Tool::ReadFile(ReadFile::default()),
                tools::Tool::ListFiles(ListFiles::default()),
            ],
            tui_tx: tx,
            db_env: env,
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
