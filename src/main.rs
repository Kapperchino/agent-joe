mod actor;
mod claude;
mod tools;

use crate::claude::{ClaudeClient, ClaudeConfig, ClientRequest, Message, Role};
use log::LevelFilter;
use ra_ap_ide::AnalysisHost;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode};
use std::env;
use std::path::PathBuf;
use tokio::main;

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

    let req =
        ClientRequest::new(vec![Message::new("Say Hellow world!".to_string())]).with_thinking();
    let res = client.chat(req).await.unwrap();
    println!("{:?}", res)
    // actor::run().await
}
