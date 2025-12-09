mod actor;
mod claude;
mod tools;

use crate::actor::{Dependency, Message, Worker};
use crate::claude::{
    ClaudeClient, ClaudeConfig, ClientRequest, Delta, StreamEvent, Tool, ToolProperty,
    ToolSchemaDTO,
};
use crate::tools::ReadFile;
use log::LevelFilter;
use ra_ap_ide::AnalysisHost;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ractor::Actor;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::path::PathBuf;
use tokio::main;
use tokio_stream::StreamExt;

pub struct Project {
    pub host: AnalysisHost,
    // keep vfs/proc_macro if you need path↔id mapping or proc-macro expansion
}

pub fn open_proj(workspace_root: impl Into<PathBuf>) -> anyhow::Result<Project> {
    let cargo_cfg = CargoConfig::default();
    let load_cfg = LoadCargoConfig {
        load_out_dirs_from_check: true,
        with_proc_macro_server: ProcMacroServerChoice::None, // or Sysroot if you need it
        prefill_caches: true,
    };
    let (db, _vfs, _pm) =
        load_workspace_at(&workspace_root.into(), &cargo_cfg, &load_cfg, &|cmd| {
            eprintln!("{cmd}")
        })?;
    Ok(Project {
        host: AnalysisHost::with_database(db),
    })
}

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

    let prompt =
        "You are an agent, given a file tools.rs, read the file and implement the enum members ";

    let (joe, actor_handle) = Actor::spawn(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![tools::Tool::ReadFile(ReadFile::default())],
        },
    )
    .await
    .expect("Failed to start actor");
    joe.send_message(Message::StartWork(prompt.to_string()))
        .unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
}
