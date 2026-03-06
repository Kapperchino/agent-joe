use actors::actor::Dependency;
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

    let api_key = std::env::var("OPENAI_KEY").expect("OPENAI_KEY must be set");
    let config = OpenAIConfig {
        api_key,
        ..Default::default()
    };
    let client = OpenAIClient::new(config).unwrap();

    let (tx, rx) = mpsc::unbounded_channel();

    let (joe, actor_handle) = Actor::spawn(
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
        },
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
