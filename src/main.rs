use actors::actor::{Dependency, Message};
use actors::supervisor::WorkerSupervisor;
use actors::worker::{Worker, WorkerAdapter};
use actors::workers::base_worker::BaseWorker;
use actors::workers::simple_worker::SimpleWorker;
use analysis::contexts::rust_context::RustContext;
use anyhow::{Context, Result};
use app::init_app::InitApp;
use app::tui::TUIApp;
use clap::Parser;
use clients::config::{Config, ConfigContext};
use clients::llm::LLmClient;
use common_models::tui_models::ActorToTui;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use flume::Sender;
use mimalloc::MiMalloc;
use ractor::{Actor, ActorRef};
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};
use std::io::stdout;
use tokio::main;
use tokio::task::JoinHandle;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

const INLINE_VIEWPORT_HEIGHT: u16 = 12;

#[derive(Parser)]
#[command(name = "joe")]
struct Cli {
    #[arg(long)]
    debug: bool,
    #[arg(long)]
    simple: bool,
    #[arg(long, help = "Override the selected model's context window")]
    context_tokens: Option<usize>,
    #[arg(long, default_value_t = 16_000)]
    response_tokens: u32,
    #[arg(long, default_value = "auto")]
    native_compaction: actors::context::NativeCompaction,
    #[arg(long, default_value = "sessions")]
    session_namespace: String,
    #[arg(long)]
    instructions_file: Option<std::path::PathBuf>,
}
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.debug {
        println!("Debug mode enabled");
    }

    let workspace = utils::workspace::WorkspacePolicy::workspace(std::env::current_dir()?)?;
    let file_appender = workspace.open_append(std::path::Path::new("logs/err.log"))?;
    let (file_appender, _guard) = tracing_appender::non_blocking(file_appender);

    let log_level = if cli.debug { Level::INFO } else { Level::WARN };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_writer(file_appender)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set the tracing subscriber")?;

    color_eyre::install().map_err(|error| anyhow::Error::from_boxed(error.into()))?;
    let terminal = ratatui::try_init_with_options(TerminalOptions {
        viewport: Viewport::Inline(INLINE_VIEWPORT_HEIGHT),
    })?;
    execute!(
        stdout(),
        EnableBracketedPaste,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .ok();

    let result = run(&cli, terminal).await;
    execute!(
        stdout(),
        SetCursorStyle::DefaultUserShape,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste
    )
    .ok();
    ratatui::restore();
    result
}

async fn run(cli: &Cli, terminal: DefaultTerminal) -> Result<()> {
    let (config, mut terminal) = match Config::load_optional()? {
        Some(config) => (config, terminal),
        None => {
            let terminal = InitApp::default().run(terminal).await?;
            let config = Config::load_optional()?.context("Setup ended without a configuration")?;
            (config, terminal)
        }
    };
    let config_context = ConfigContext::new(config.prepare().await?);
    let (tx, rx) = flume::unbounded();
    let RunningActor {
        actor: joe,
        handle: actor_handle,
    } = if cli.simple {
        get_actor(cli, SimpleWorker::new(), tx, config_context.clone()).await
    } else {
        get_actor(cli, BaseWorker::new(), tx, config_context.clone()).await
    }?;

    terminal.clear().ok();
    let app = TUIApp::new(joe.clone(), config_context, cli.debug);
    let result = app
        .run(terminal, rx)
        .await
        .map_err(|error| anyhow::Error::from_boxed(error.into()));
    if result.is_err() {
        let _ = joe.send_message(Message::KYS);
    }
    let stopped = actor_handle.await.context("Actor failed to exit cleanly");
    result.and(stopped)
}

struct RunningActor {
    actor: ActorRef<Message>,
    handle: JoinHandle<()>,
}

async fn get_actor<W: Worker<C = RustContext>>(
    cli: &Cli,
    worker: W,
    chan: Sender<ActorToTui>,
    config_context: ConfigContext,
) -> Result<RunningActor> {
    let mut runtime = actors::runtime::Runtime::with_session_namespace(
        std::env::current_dir()?,
        &cli.session_namespace,
    )?;
    runtime.context_budget =
        actors::context::ContextBudget::new(cli.context_tokens, cli.response_tokens)?;
    runtime
        .context_budget
        .resolve(config_context.get_config().context_window())?;
    runtime.native_compaction = cli.native_compaction;
    let workspace = runtime.scope.workspace()?;
    let mut context = runtime
        .scope
        .enter(RustContext::new(
            W::init_prompt(None),
            0,
            workspace.root().to_path_buf(),
        ))
        .await
        .context("Failed to initialize project analysis")?;
    if let Some(path) = cli
        .instructions_file
        .clone()
        .or_else(|| dirs::home_dir().map(|home| home.join(".turbo-code/AGENTS.md")))
    {
        context.guidance = context.guidance.with_global(path)?;
    }
    context.initial_prompt.push_str(
        "\nAll repository operations must remain inside the project. File tools run through the project filesystem policy. Cargo automatically downloads missing crates.io dependencies through the host before running inside the project sandbox. These dependency downloads are allowed and require no permission request. Project code has no network access, and Cargo fails if isolation is unavailable. Other outside access is denied automatically; do not request permissions or broader access.",
    );
    let client = LLmClient::new(config_context)?;
    let (supervisor, _) = Actor::spawn(None, WorkerSupervisor, ())
        .await
        .context("Failed to start supervisor")?;
    let (actor, handle) = Actor::spawn_linked(
        None,
        WorkerAdapter::new(worker),
        Dependency {
            runtime,
            client,
            tools: W::tools(),
            tui_tx: chan,
            debug_mode: cli.debug,
            context,
        },
        supervisor.get_cell(),
    )
    .await
    .context("Failed to start actor")?;
    Ok(RunningActor { actor, handle })
}
