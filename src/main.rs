use actors::actor::Dependency;
use actors::supervisor::WorkerSupervisor;
use actors::worker::Worker;
use app::app::App;
use app::init_app::InitApp;
use clients::config::Config;
use clients::llm::LLmClient;
use clients::tool_defs::CargoCheck;
use clients::tool_defs::InsertAfterLine;
use clients::tool_defs::ReadFile;
use clients::tool_defs::StringReplace;
use clients::tool_defs::Tool;
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use ractor::Actor;
use ratatui::{TerminalOptions, Viewport};
use std::io::stdout;
use tokio::main;
use tokio::sync::mpsc;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::FmtSubscriber;

const INLINE_VIEWPORT_HEIGHT: u16 = 12;

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

    color_eyre::install().unwrap();
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
    });
    execute!(stdout(), EnableBracketedPaste).ok();

    let config = match Config::load_optional().unwrap() {
        Some(config) => config,
        None => {
            let (returned_terminal, config) = InitApp::default().run(terminal).await.unwrap();
            terminal = returned_terminal;
            config
        }
    }
    .prepare()
    .await
    .unwrap();

    let client = LLmClient::new(config).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();

    let (supervisor, _) = Actor::spawn(None, WorkerSupervisor, ())
        .await
        .expect("Failed to start supervisor");

    let (joe, actor_handle) = Actor::spawn_linked(
        None,
        Worker {},
        Dependency {
            claude: client,
            tools: vec![
                Tool::ReadFile(ReadFile::default()),
                Tool::InsertAfterLine(InsertAfterLine::default()),
                Tool::StringReplace(StringReplace::default()),
                Tool::CargoCheck(CargoCheck::default()),
                Tool::Grep(clients::tool_defs::GrepTool::default()),
            ],
            tui_tx: tx,
            log_streams: false,
        },
        supervisor.get_cell(),
    )
    .await
    .expect("Failed to start actor");

    terminal.clear().ok();
    let app = App::new(joe);
    app.run(terminal, rx).await.unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
    execute!(stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
}
