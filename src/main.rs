use actors::actor::Dependency;
use actors::supervisor::WorkerSupervisor;
use actors::worker::Worker;
use app::app::App;
use clients::llm::LLmClient;
use clients::openai::{OpenAIClient, OpenAIConfig};
use clients::tool_defs::CargoCheck;
use clients::tool_defs::InsertAfterLine;
use clients::tool_defs::ReadFile;
use clients::tool_defs::StringReplace;
use clients::tool_defs::Tool;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use ractor::Actor;
use std::io::{stdout, Write};
use tokio::main;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::FmtSubscriber;

const CHATGPT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

struct ResolvedAuth {
    api_key: String,
    base_url: String,
    extra_headers: Vec<(String, String)>,
}

/// Try to read an OpenAI credential from ~/.codex/auth.json (written by `codex login`).
/// Supports ChatGPT OAuth tokens and API key formats.
fn read_codex_auth() -> Option<ResolvedAuth> {
    let home = dirs::home_dir()?;
    let auth_path = home.join(".codex").join("auth.json");
    let contents = std::fs::read_to_string(&auth_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;

    // ChatGPT OAuth login: { "auth_mode": "chatgpt", "tokens": { "access_token": "...", "account_id": "..." } }
    if json.get("auth_mode").and_then(|v| v.as_str()) == Some("chatgpt") {
        if let Some(tokens) = json.get("tokens") {
            if let Some(token) = tokens.get("access_token").and_then(|v| v.as_str()) {
                let mut extra_headers = vec![];
                if let Some(account_id) = tokens.get("account_id").and_then(|v| v.as_str()) {
                    extra_headers.push(("ChatGPT-Account-Id".to_string(), account_id.to_string()));
                }
                eprintln!("Using ChatGPT OAuth token from ~/.codex/auth.json");
                return Some(ResolvedAuth {
                    api_key: token.to_string(),
                    base_url: CHATGPT_CODEX_BASE_URL.to_string(),
                    extra_headers,
                });
            }
        }
    }

    // API key stored directly: { "OPENAI_API_KEY": "sk-..." }
    if let Some(key) = json.get("OPENAI_API_KEY").and_then(|v| v.as_str()) {
        eprintln!("Using API key from ~/.codex/auth.json");
        return Some(ResolvedAuth {
            api_key: key.to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            extra_headers: vec![],
        });
    }

    None
}

/// Resolve auth: OPENAI_KEY env var > ~/.codex/auth.json
fn resolve_auth() -> ResolvedAuth {
    // if let Ok(key) = std::env::var("OPENAI_KEY") {
    //     return ResolvedAuth {
    //         api_key: key,
    //         base_url: "https://api.openai.com/v1".to_string(),
    //     };
    // }
    if let Some(auth) = read_codex_auth() {
        return auth;
    }
    panic!("No OpenAI API key found. Set OPENAI_KEY or run `codex login`.");
}

#[main]
async fn main() {
    let file_appender = RollingFileAppender::new(Rotation::HOURLY, "./logs", "err.log");
    let (file_appender, _guard) = tracing_appender::non_blocking(file_appender);

    // a builder for `FmtSubscriber`.
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::WARN)
        .with_writer(file_appender)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let auth = resolve_auth();
    let config = OpenAIConfig {
        api_key: auth.api_key,
        url: auth.base_url,
        extra_headers: auth.extra_headers,
        model: "gpt-5.4-mini".to_string(),
        ..Default::default()
    };
    let client = OpenAIClient::new(config).unwrap();

    let (tx, rx) = mpsc::unbounded_channel();

    let (supervisor, _) = Actor::spawn(None, WorkerSupervisor, ())
        .await
        .expect("Failed to start supervisor");

    let (joe, actor_handle) = Actor::spawn_linked(
        None,
        Worker {},
        Dependency {
            claude: LLmClient::OpenApi(client),
            tools: vec![
                Tool::ReadFile(ReadFile::default()),
                Tool::InsertAfterLine(InsertAfterLine::default()),
                Tool::StringReplace(StringReplace::default()),
                Tool::CargoCheck(CargoCheck::default()),
            ],
            tui_tx: tx,
            save_stream: true,
        },
        supervisor.get_cell(),
    )
    .await
    .expect("Failed to start actor");

    color_eyre::install().unwrap();
    let terminal = ratatui::init();
    execute!(stdout(), EnableBracketedPaste).ok();
    let app = App::new(joe);
    let app_result = app.run(terminal, rx).await.unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
    execute!(stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
}
