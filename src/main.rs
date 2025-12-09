mod actor;
mod claude;
mod tools;
mod tui;

use crate::actor::{Dependency, Message, Worker};
use crate::claude::{ClaudeClient, ClaudeConfig};
use crate::tools::ReadFile;
use crate::tui::UiEvent;
use log::LevelFilter;
use ra_ap_ide::AnalysisHost;
use ra_ap_load_cargo::{load_workspace_at, LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_project_model::CargoConfig;
use ractor::Actor;
use simplelog::{CombinedLogger, Config, WriteLogger};
use std::env;
use std::fs::File;
use std::path::PathBuf;
use tokio::main;
use tokio::sync::mpsc;

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
async fn main() -> anyhow::Result<()> {
    // Use file-based logging so it doesn't interfere with the TUI
    let log_file = File::create("turbo-code.log").unwrap_or_else(|_| {
        File::options()
            .write(true)
            .truncate(true)
            .open("turbo-code.log")
            .expect("Unable to open log file")
    });

    CombinedLogger::init(vec![WriteLogger::new(
        LevelFilter::Info,
        Config::default(),
        log_file,
    )])
    .ok();

    let client = ClaudeClient::new(ClaudeConfig {
        api_key: env::var("CLAUDE_API").expect("CLAUDE_API environment variable not set"),
        ..Default::default()
    })?;

    // Create channels for TUI <-> Actor communication
    let (user_tx, mut user_rx) = mpsc::unbounded_channel::<String>();
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiEvent>();

    // Spawn the actor
    let (actor_ref, actor_handle) = Actor::spawn(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![tools::Tool::ReadFile(ReadFile::default())],
            ui_tx: ui_tx.clone(),
        },
    )
    .await
    .expect("Failed to start actor");

    // Spawn a task to forward user input from TUI to actor
    let actor_for_input = actor_ref.clone();
    tokio::spawn(async move {
        while let Some(prompt) = user_rx.recv().await {
            if actor_for_input
                .send_message(Message::StartWork(Some(prompt)))
                .is_err()
            {
                break;
            }
        }
    });

    // Run the TUI (this blocks until the user quits)
    if let Err(e) = tui::run_tui(user_tx, ui_rx).await {
        eprintln!("TUI error: {}", e);
    }

    // Cleanup: stop the actor
    actor_ref.stop(None);

    // Wait for actor to finish
    let _ = actor_handle.await;

    Ok(())
}
