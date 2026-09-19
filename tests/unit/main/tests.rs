use super::*;
use clients::{LocalOpenAIConfig, OpenAIAuthConfig, OpenAIConfig, OpenAIEffort};
use std::{path::PathBuf, time::Duration};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("joe-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let fixture = Self {
            root: root.canonicalize()?,
        };
        std::fs::write(fixture.root.join("target"), "sandbox unavailable")?;
        std::fs::write(fixture.root.join("main.rs"), "fn main() {}")?;
        std::fs::write(fixture.root.join("AGENTS.md"), "Fixture instructions")?;
        Ok(fixture)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

async fn startup_without_sandbox<W: ContextWorker<C = RustContext>>(worker: W) -> Result<()> {
    let fixture = Fixture::new()?;
    let mut cli = Cli::try_parse_from(["joe"])?;
    cli.instructions_file = Some(fixture.root.join("AGENTS.md"));
    let config = ConfigContext::new(Config::OpenAI(OpenAIConfig {
        auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
            api_key: None,
            url: "http://127.0.0.1:1/v1".into(),
        }),
        model: "fixture".into(),
        effort: OpenAIEffort::None,
        request_encrypted_reasoning: None,
    }));
    let (tx, _rx) = flume::unbounded();
    let running = tokio::time::timeout(
        Duration::from_secs(10),
        get_actor(&cli, worker, tx, config, fixture.root.clone()),
    )
    .await
    .context("Actor startup did not finish")??;
    let no_sandbox_tasks = running.scope.tasks.is_empty();
    let readable = running
        .scope
        .workspace()?
        .read(std::path::Path::new("main.rs"))?;
    let mut command = tokio::process::Command::new("cargo");
    command.arg("--version");
    let execution = tokio::time::timeout(
        Duration::from_secs(10),
        running
            .scope
            .sandbox()?
            .capture(command, Default::default(), Vec::new()),
    )
    .await;
    running.actor.send_message(Message::KYS)?;
    tokio::time::timeout(Duration::from_secs(10), running.handle).await??;
    tokio::time::timeout(Duration::from_secs(10), running.scope.finish()).await?;

    assert!(no_sandbox_tasks, "Startup must not boot a sandbox VM");
    assert_eq!(readable, "fn main() {}");
    let error = execution?
        .err()
        .context("Execution must fail when sandbox initialization is unavailable")?;
    assert!(
        error
            .to_string()
            .contains("Cannot create the process temporary directory"),
        "{error:#}"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.join("target"))?,
        "sandbox unavailable"
    );
    Ok(())
}

#[tokio::test]
async fn base_worker_startup_does_not_initialize_sandbox() -> Result<()> {
    startup_without_sandbox(BaseWorker::new()).await
}

#[tokio::test]
async fn simple_worker_startup_does_not_initialize_sandbox() -> Result<()> {
    startup_without_sandbox(SimpleWorker::new()).await
}
