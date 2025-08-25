mod dumbass;

use ra_ap_ide::AnalysisHost;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use std::path::PathBuf;
use claude_api::{ClaudeClient, ClaudeConfig, Message, MessageBuilder, models_constants};
use std::env;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Example usage of the Claude API client
    
    // Try to get API key from environment variable
    let api_key = match env::var("CLAUDE_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("Please set the CLAUDE_API_KEY environment variable to use this example");
            return Ok(());
        }
    };

    // Create a client with a custom configuration
    let config = ClaudeConfig::new(api_key)?
        .with_anthropic_version("2023-06-01");
    
    let client = ClaudeClient::new(config)?;

    // Example 1: Simple message
    println!("=== Simple Message Example ===");
    let messages = vec![
        Message::user("Hello! Can you tell me a short joke?")
    ];

    match client.create_message_simple(models_constants::CLAUDE_3_HAIKU, 150, messages).await {
        Ok(response) => {
            println!("Claude's response: {:?}", response.content);
            println!("Usage: {} input tokens, {} output tokens", 
                response.usage.input_tokens, response.usage.output_tokens);
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }

    println!("\n=== Message Builder Example ===");
    // Example 2: Using the message builder with system prompt
    let request = MessageBuilder::new(models_constants::CLAUDE_3_HAIKU, 200)
        .system("You are a helpful coding assistant")
        .user_message("Write a simple hello world function in Rust")
        .temperature(0.3)
        .build();

    match client.create_message(request).await {
        Ok(response) => {
            println!("Claude's coding response: {:?}", response.content);
        }
        Err(e) => {
            println!("Error: {}", e);
        }
    }

    Ok(())
}
