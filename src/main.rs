mod actor;
mod claude;
mod cur_context;
mod tools;
mod worker;
mod actor_state;
mod app;

use crate::actor::{Dependency, Message};
use crate::claude::{ClaudeClient, ClaudeConfig};
use crate::tools::ReadFile;
use crate::worker::Worker;
use log::LevelFilter;
use ractor::Actor;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::env;
use std::io::Write;
use tokio::main;
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

    // let prompt =
    //     "You are an agent, given a file tools.rs, read the file and implement the enum members ";
    //
    // let (joe, actor_handle) = Actor::spawn(
    //     None,
    //     Worker {},
    //     Dependency {
    //         claude: client,
    //         tools: vec![tools::Tool::ReadFile(ReadFile::default())],
    //     },
    // )
    // .await
    // .expect("Failed to start actor");
    // joe.send_message(Message::StartWork(Some(prompt.to_string())))
    //     .unwrap();
    // actor_handle.await.expect("Actor failed to exit cleanly");

    color_eyre::install().unwrap();
    let terminal = ratatui::init();
    let app_result = crate::app::App::new().run(terminal).await.unwrap();
    ratatui::restore();

}
