use actors::actor::Dependency;
use actors::supervisor::WorkerSupervisor;
use actors::worker::{Worker, WorkerAdapter};
use actors::workers::base_worker::BaseWorker;
use analysis::contexts::rust_context::RustContext;
use app::init_app::InitApp;
use app::tui::TUIApp;
use clap::Parser;
use clients::config::{Config, ConfigContext};
use clients::llm::LLmClient;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
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

#[derive(Parser)]
#[command(name = "joe")]
struct Cli {
    #[arg(long)]
    debug: bool,
}

#[main]
async fn main() {
    let cli = Cli::parse();
    if cli.debug {
        println!("Debug mode enabled");
    }
    let file_appender = RollingFileAppender::new(Rotation::HOURLY, "./logs", "err.log");
    let (file_appender, _guard) = tracing_appender::non_blocking(file_appender);

    let log_level = if cli.debug { Level::INFO } else { Level::WARN };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_writer(file_appender)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    color_eyre::install().unwrap();
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
    });
    execute!(
        stdout(),
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )
    .ok();

    let config = Config::load_optional().unwrap();
    let (config, mut terminal) = if config.is_none() {
        let continue_term = InitApp::default().run(terminal).await.unwrap();
        let config = Config::load_optional()
            .unwrap()
            .unwrap()
            .prepare()
            .await
            .unwrap();
        (config, continue_term)
    } else {
        (config.unwrap(), terminal)
    };
    let config_context = ConfigContext::new(config);

    let client = LLmClient::new(config_context.clone()).unwrap();
    let (tx, rx) = mpsc::unbounded_channel();

    let (supervisor, _) = Actor::spawn(None, WorkerSupervisor, ())
        .await
        .expect("Failed to start supervisor");

    let context = RustContext::new("You are a rust coding orchestrator agent in a rust codebase, \
    you do not have any direct read or write abilities, but you are able to spawn other agents to do the job for you.\
    You also have every symbol in this project in your context, use the information if it is present.
    Keep the commands concise and accurate.".to_owned())
        .await
        .unwrap();

    let (joe, actor_handle) = Actor::spawn_linked(
        None,
        WorkerAdapter::new(BaseWorker::new()),
        Dependency {
            client,
            tools: BaseWorker::tools(),
            tui_tx: tx,
            debug_mode: cli.debug,
            context,
        },
        supervisor.get_cell(),
    )
    .await
    .expect("Failed to start actor");

    terminal.clear().ok();
    let app = TUIApp::new(joe, config_context.clone(), cli.debug);
    app.run(terminal, rx).await.unwrap();
    actor_handle.await.expect("Actor failed to exit cleanly");
    execute!(
        stdout(),
        SetCursorStyle::DefaultUserShape,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste
    )
    .ok();
    ratatui::restore();
}
