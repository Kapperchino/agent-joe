use super::*;
use crate::workers::{base_worker::BaseWorker, simple_worker::SimpleWorker};
use analysis::contexts::rust_context::RustContext;

#[derive(Clone, Copy)]
enum Mode {
    Simple,
    Delegated,
}

#[test]
fn workers_expose_one_cargo_tool_with_their_allowed_operations() {
    use crate::workers::{validate_worker::ValidateWorker, write_worker::WriteWorker};
    use analysis::contexts::{context::Context, rust_empty_context::RustEmptyContext};

    fn operations<C: Context>(tools: Vec<ErasedToolRef<C, ActorContext<C>>>) -> Value {
        let cargo = tools
            .into_iter()
            .filter(|tool| {
                let name = tool.name();
                name.starts_with("cargo") || name.starts_with("process_")
            })
            .collect::<Vec<_>>();
        assert_eq!(cargo.len(), 1);
        match cargo[0].definition() {
            ToolDefinition::Client {
                name,
                properties,
                required,
                ..
            } => {
                assert_eq!(name, "cargo");
                assert_eq!(required, ["operation"]);
                match &properties["operation"] {
                    tools::tool_defs::ToolProperty::Schema(schema) => schema["enum"].clone(),
                    _ => panic!("Cargo requires a typed operation schema"),
                }
            }
            _ => panic!("Cargo must be a client tool"),
        }
    }

    let simple = operations(SimpleWorker::<RustContext>::tools());
    let validation = operations(ValidateWorker::<RustEmptyContext>::tools());
    let writing = operations(WriteWorker::<RustEmptyContext>::tools());
    assert_eq!(
        simple,
        json!([
            "check",
            "test",
            "fmt",
            "fmt_check",
            "clippy",
            "run",
            "start",
            "poll",
            "stop"
        ])
    );
    assert_eq!(
        validation,
        json!([
            "check",
            "test",
            "fmt_check",
            "clippy",
            "run",
            "start",
            "poll",
            "stop"
        ])
    );
    assert_eq!(writing, simple);
}

struct RepositoryActor {
    actor: ActorRef<Message>,
    handle: tokio::task::JoinHandle<()>,
    requests: flume::Receiver<Request>,
    events: flume::Receiver<ActorToTui>,
    context: RustContext,
    store: Arc<crate::session::SessionStore>,
}

impl RepositoryActor {
    async fn new<W: Worker<C = RustContext>>(worker: W, root: std::path::PathBuf) -> Self {
        Self::configured(worker, root, false).await
    }

    async fn configured<W: Worker<C = RustContext>>(
        worker: W,
        root: std::path::PathBuf,
        debug_mode: bool,
    ) -> Self {
        let runtime = Runtime::for_workspace(root.clone()).unwrap();
        let context = runtime
            .scope
            .enter(RustContext::new(W::init_prompt(None), 0, root))
            .await
            .unwrap();
        let (tx, requests) = flume::unbounded();
        let (tui_tx, events) = flume::unbounded();
        let store = runtime.sessions.clone().unwrap();
        let (actor, handle) = Actor::spawn(
            None,
            WorkerAdapter::new(worker),
            Dependency {
                client: llm::LLmClient::Injected(Arc::new(Provider(tx))),
                tools: W::tools(),
                tui_tx,
                debug_mode,
                context: context.clone(),
                runtime,
            },
        )
        .await
        .unwrap();
        Self {
            actor,
            handle,
            requests,
            events,
            context,
            store,
        }
    }

    async fn request(&self) -> Request {
        within(self.requests.recv_async()).await.unwrap()
    }

    async fn event(&self, predicate: impl Fn(&ActorToTui) -> bool) -> ActorToTui {
        within(async {
            let mut found = None;
            while found.is_none() {
                let event = self.events.recv_async().await.unwrap();
                found = predicate(&event).then_some(event);
            }
            found.unwrap()
        })
        .await
    }

    async fn stop(self) {
        let children = self.actor.get_cell().get_children();
        self.actor.stop(None);
        within(self.handle).await.unwrap();
        for child in children {
            assert_eq!(child.get_status(), ractor::ActorStatus::Stopped);
        }
    }
}

fn tool(name: &str, id: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolBlock {
        tool_id: ToolId {
            id: id.to_owned().try_into().unwrap(),
            call_id: None,
        },
        name: name.to_owned().try_into().unwrap(),
        input: input.as_object().unwrap().clone(),
    }
}

fn result_text(request: &llm::ClientRequest) -> String {
    request
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn simple_and_delegated_turns_receive_scoped_rules_before_editing_and_read_fresh_files() {
    for mode in [Mode::Simple, Mode::Delegated] {
        let workspace = crate::session::tests::Workspace::new();
        std::fs::create_dir_all(workspace.path.join("docs")).unwrap();
        std::fs::write(
            workspace.path.join("AGENTS.md"),
            "Repository guidance marker",
        )
        .unwrap();
        std::fs::write(
            workspace.path.join("docs/AGENTS.md"),
            "Nested Markdown marker",
        )
        .unwrap();
        let actor = match mode {
            Mode::Simple => RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await,
            Mode::Delegated => {
                RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await
            }
        };
        assert_eq!(actor.actor.get_cell().get_children().len(), 1);
        actor
            .actor
            .send_message(Message::StartWork(Some(
                "Create docs/new.md following scoped rules".into(),
            )))
            .unwrap();
        let (initial, reply) = actor.request().await;
        assert!(
            initial
                .system
                .as_ref()
                .unwrap()
                .contains("Repository guidance marker")
        );
        assert!(
            !initial
                .system
                .as_ref()
                .unwrap()
                .contains("Nested Markdown marker")
        );
        let reply = match mode {
            Mode::Simple => reply,
            Mode::Delegated => {
                answer(
                    reply,
                    response(vec![tool(
                        "make_changes",
                        "delegate",
                        json!({"context": "Create docs/new.md following scoped rules"}),
                    )]),
                );
                let (worker, reply) = actor.request().await;
                assert!(
                    worker
                        .system
                        .as_ref()
                        .unwrap()
                        .contains("Repository guidance marker")
                );
                reply
            }
        };
        let patch = json!({"patch": "*** Begin Patch\n*** Add File: docs/new.md\n+αβ\n+second\n*** End Patch"});
        answer(
            reply,
            response(vec![tool("apply_patch", "unseen-rules", patch.clone())]),
        );
        let (guarded, reply) = actor.request().await;
        assert!(!workspace.path.join("docs/new.md").exists());
        assert!(
            guarded
                .system
                .as_ref()
                .unwrap()
                .contains("Nested Markdown marker")
        );
        assert!(result_text(&guarded).contains("Scoped instructions must be received"));
        answer(
            reply,
            response(vec![tool("apply_patch", "scoped-edit", patch)]),
        );
        let (edited, reply) = actor.request().await;
        assert_eq!(
            std::fs::read_to_string(workspace.path.join("docs/new.md")).unwrap(),
            "αβ\nsecond"
        );
        assert!(result_text(&edited).contains("ok"));
        answer(
            reply,
            response(vec![tool("review_changes", "review-task", json!({}))]),
        );
        let (reviewed, reply) = actor.request().await;
        assert!(result_text(&reviewed).contains("task_diff"));
        assert!(result_text(&reviewed).contains("docs/new.md"));
        let snapshots = actor.store.list().unwrap();
        let root = snapshots
            .iter()
            .find(|snapshot| snapshot.parent.is_none())
            .unwrap();
        assert!(root.changes.baseline.is_some());
        assert_eq!(root.changes.records.len(), 1);
        answer(
            reply,
            response(vec![tool(
                "read_file",
                "read-new",
                json!({"file_path": "docs/new.md", "range": {"start": 1, "end": 3}}),
            )]),
        );
        let (read, reply) = actor.request().await;
        assert!(result_text(&read).contains("1: αβ\n2: second"));
        answer(reply, response(vec![text("Created the Markdown file.")]));
        if matches!(mode, Mode::Delegated) {
            let (parent, reply) = actor.request().await;
            assert!(
                !parent
                    .system
                    .as_ref()
                    .unwrap()
                    .contains("Nested Markdown marker")
            );
            assert_eq!(
                result_text(&parent)
                    .matches("Created the Markdown file.")
                    .count(),
                1
            );
            answer(reply, response(vec![text("Completed.")]));
        }
        actor
            .event(|event| {
                event.actor_id == 0
                    && matches!(
                        event.packet,
                        ActorToTuiPacket::TurnChanged {
                            state: Lifecycle::Completed,
                            ..
                        }
                    )
            })
            .await;
        actor
            .actor
            .send_message(Message::Command(commands::command::Command::PrintContext))
            .unwrap();
        let event = actor
            .event(|event| {
                matches!(
                    event.packet,
                    ActorToTuiPacket::CommandResult(commands::command::Command::PrintContext, _)
                )
            })
            .await;
        match event.packet {
            ActorToTuiPacket::CommandResult(_, report) => {
                let report: Value = serde_json::from_str(&report).unwrap();
                assert_eq!(report["instruction_truncated"], false);
                assert_eq!(report["symbol_map_injected"], false);
                assert_eq!(report["worker_summaries_injected"], false);
                assert!(
                    report["inventory"]["sample"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("docs/new.md"))
                );
            }
            _ => panic!("Expected inspector result"),
        }
        let saved = actor
            .store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.parent.is_none())
            .unwrap()
            .id;
        actor
            .actor
            .send_message(Message::Command(commands::command::Command::New))
            .unwrap();
        actor
            .event(|event| {
                matches!(
                    event.packet,
                    ActorToTuiPacket::CommandResult(commands::command::Command::New, _)
                )
            })
            .await;
        std::fs::write(
            workspace.path.join("AGENTS.md"),
            "Refreshed repository marker",
        )
        .unwrap();
        actor
            .actor
            .send_message(Message::Command(commands::command::Command::Resume(
                commands::command::ResumeTarget::Session { id: saved },
            )))
            .unwrap();
        actor
            .event(|event| matches!(event.packet, ActorToTuiPacket::SessionResumed(Ok(_))))
            .await;
        assert!(actor.requests.is_empty());
        actor
            .actor
            .send_message(Message::StartWork(Some(
                "Report the current guidance".into(),
            )))
            .unwrap();
        let (resumed, reply) = actor.request().await;
        assert!(
            resumed
                .system
                .as_ref()
                .unwrap()
                .contains("Refreshed repository marker")
        );
        assert!(
            !resumed
                .system
                .as_ref()
                .unwrap()
                .contains("Repository guidance marker")
        );
        answer(reply, response(vec![text("Current guidance received.")]));
        actor
            .event(|event| {
                event.actor_id == 0
                    && matches!(
                        event.packet,
                        ActorToTuiPacket::TurnChanged {
                            state: Lifecycle::Completed,
                            ..
                        }
                    )
            })
            .await;
        actor.stop().await;
    }
}

#[tokio::test]
async fn shared_watcher_handles_create_modify_rename_and_delete_in_both_root_modes() {
    for mode in [Mode::Simple, Mode::Delegated] {
        let workspace = crate::session::tests::Workspace::new();
        let actor = match mode {
            Mode::Simple => RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await,
            Mode::Delegated => {
                RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await
            }
        };
        let watcher = actor
            .actor
            .get_cell()
            .get_children()
            .into_iter()
            .next()
            .unwrap();
        let project = actor.context.rust_proj.clone();
        let directory = project.workspace().root().join("logs");
        std::fs::create_dir_all(&directory).unwrap();
        let first = directory.join("first.rs");
        let second = directory.join("second.rs");
        std::fs::write(&first, "pub fn initial() {}\n").unwrap();
        let refresh = |paths| {
            watcher
                .send_message(crate::background_actors::file_actor::Message::Changed(
                    paths,
                ))
                .unwrap();
            watcher
                .send_message(crate::background_actors::file_actor::Message::ApplyVFS)
                .unwrap();
        };
        refresh(vec![first.clone()]);
        within(async {
            while project.get_file_id(first.clone()).is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        std::fs::write(&first, "pub fn updated() {}\n").unwrap();
        refresh(vec![first.clone()]);
        within(async {
            while !project
                .get_all_proj_symbols()
                .await
                .unwrap()
                .iter()
                .any(|symbol| symbol.name == "updated")
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
        std::fs::rename(&first, &second).unwrap();
        refresh(vec![first.clone(), second.clone()]);
        within(async {
            while project.get_file_id(first.clone()).is_some()
                || project.get_file_id(second.clone()).is_none()
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
        std::fs::remove_file(&second).unwrap();
        refresh(vec![second.clone()]);
        within(async {
            while project.get_file_id(second.clone()).is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        actor.stop().await;
    }
}

#[path = "worker_runtime_test.rs"]
mod worker_tests;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn simple_and_validation_workers_manage_targets_and_archive_completion() {
    if utils::test_support::sandbox_available() {
        for mode in [Mode::Simple, Mode::Delegated] {
            let workspace = crate::session::tests::Workspace::new();
            std::fs::create_dir(workspace.path.join("examples")).unwrap();
            std::fs::write(
                workspace.path.join("Cargo.toml"),
                "[package]\nname = 'managed_fixture'\nversion = '0.1.0'\nedition = '2024'\n",
            )
            .unwrap();
            std::fs::write(workspace.path.join("examples/server.rs"), "fn main() {\n    use std::io::Write;\n    println!(\"ready\");\n    std::io::stdout().flush().unwrap();\n    std::fs::write(\"ready\", \"ready\").unwrap();\n    loop { std::thread::sleep(std::time::Duration::from_millis(50)); }\n}\n").unwrap();
            let actor = match mode {
                Mode::Simple => {
                    RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await
                }
                Mode::Delegated => {
                    RepositoryActor::new(BaseWorker::new(), workspace.path.clone()).await
                }
            };
            actor
                .actor
                .send_message(Message::StartWork(Some(
                    "Exercise a managed example and stop it".into(),
                )))
                .unwrap();
            let (_, reply) = actor.request().await;
            let reply = match mode {
                Mode::Simple => reply,
                Mode::Delegated => {
                    answer(
                        reply,
                        response(vec![tool(
                            "validate_rust",
                            "delegate-validation",
                            json!({"context":"Exercise a managed example and stop it"}),
                        )]),
                    );
                    actor.request().await.1
                }
            };
            answer(
                reply,
                response(vec![tool(
                    "cargo",
                    "start",
                    json!({"operation":"start","target":{"kind":"example","name":"server"}}),
                )]),
            );
            let (started, reply) = actor.request().await;
            let result = latest_cargo(&started);
            assert_eq!(result.status, utils::process::ProcessStatus::Running);
            let id = result.process_id.unwrap();
            tokio::time::timeout(Duration::from_secs(20), async {
                while !workspace.path.join("ready").exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            answer(
                reply,
                response(vec![tool(
                    "cargo",
                    "blocked-check",
                    json!({"operation":"check"}),
                )]),
            );
            let (blocked, reply) = actor.request().await;
            assert!(result_text(&blocked).contains("Stop the managed target"));
            answer(
                reply,
                response(vec![tool(
                    "cargo",
                    "poll",
                    json!({"operation":"poll","process_id":id}),
                )]),
            );
            let (polled, reply) = actor.request().await;
            let result = latest_cargo(&polled);
            assert!(result.stdout.content.contains("ready"));
            answer(
                reply,
                response(vec![tool(
                    "cargo",
                    "stop",
                    json!({"operation":"stop","process_id":id}),
                )]),
            );
            let (stopped, reply) = actor.request().await;
            assert_eq!(
                latest_cargo(&stopped).status,
                utils::process::ProcessStatus::Cancelled
            );
            answer(
                reply,
                response(vec![tool(
                    "cargo",
                    "check",
                    json!({"operation":"check","target":{"kind":"example","name":"server"}}),
                )]),
            );
            let (checked, reply) = actor.request().await;
            let result = latest_cargo(&checked);
            assert!(!result.is_error(), "{result:?}");
            assert_eq!(result.workspace_revision, Some(1));
            answer(
                reply,
                response(vec![text(
                    "Example exercised and stopped; targeted compilation passed.",
                )]),
            );
            if matches!(mode, Mode::Delegated) {
                answer(
                    actor.request().await.1,
                    response(vec![text("Validation reported by the validation worker.")]),
                );
            }
            actor
                .event(|event| {
                    event.actor_id == 0
                        && matches!(
                            event.packet,
                            ActorToTuiPacket::TurnChanged {
                                state: Lifecycle::Completed,
                                ..
                            }
                        )
                })
                .await;
            let sessions = actor.store.list().unwrap();
            let process = sessions
                .iter()
                .find_map(|session| session.processes.get(&id))
                .unwrap();
            assert_eq!(process.status, utils::process::ProcessStatus::Cancelled);
            assert!(process.stdout.content.contains("ready"));
            actor.stop().await;
        }
    }
}

fn latest_cargo(request: &llm::ClientRequest) -> utils::cargo::CargoResult {
    request
        .messages
        .iter()
        .rev()
        .flat_map(|message| message.content.iter().rev())
        .find_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => serde_json::from_str(content).ok(),
            _ => None,
        })
        .unwrap_or_else(|| panic!("No Cargo result: {:?}", request.messages.last()))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn typed_tools_reproduce_patch_and_verify_a_rust_regression() {
    if utils::test_support::sandbox_available() {
        let workspace = crate::session::tests::Workspace::new();
        std::fs::create_dir(workspace.path.join("src")).unwrap();
        std::fs::write(workspace.path.join("Cargo.toml"), "[package]\nname = 'regression_fixture'\nversion = '0.1.0'\nedition = '2024'\n[features]\nregression = []\n").unwrap();
        std::fs::write(workspace.path.join("src/lib.rs"), "pub fn answer() -> u32 { 41 }\n#[cfg(all(test, feature = \"regression\"))]\nmod tests {\n    #[test]\n    fn answer() { assert_eq!(super::answer(), 42); }\n}\n").unwrap();
        let actor = RepositoryActor::new(SimpleWorker::new(), workspace.path.clone()).await;
        actor
            .actor
            .send_message(Message::StartWork(Some(
                "Reproduce and fix the answer regression, preserving its feature gate".into(),
            )))
            .unwrap();
        let selection = json!({"operation":"test","target":{"kind":"lib"},"features":["regression"],"test_name":"tests::answer","exact":true});
        answer(
            actor.request().await.1,
            response(vec![tool("cargo", "reproduce", selection.clone())]),
        );
        let (failed, reply) = actor.request().await;
        let failure = result_text(&failed);
        assert!(failure.contains("tests::answer"));
        assert!(failure.contains("FAILED"));
        answer(
            reply,
            response(vec![tool(
                "apply_patch",
                "fix",
                json!({"patch":"*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-pub fn answer() -> u32 { 41 }\n+pub fn answer() -> u32 { 42 }\n*** End Patch"}),
            )]),
        );
        let (patched, reply) = actor.request().await;
        assert!(result_text(&patched).contains("ok"));
        answer(reply, response(vec![tool("cargo", "verify", selection)]));
        let (verified, reply) = actor.request().await;
        let result = latest_cargo(&verified);
        assert!(!result.is_error(), "{result:?}");
        assert!(result.stdout.content.contains("1 passed"));
        assert!(!result.reused);
        assert_eq!(result.workspace_revision, Some(1));
        answer(
            reply,
            response(vec![tool("cargo", "format", json!({"operation":"fmt"}))]),
        );
        let (formatted, reply) = actor.request().await;
        assert!(!latest_cargo(&formatted).is_error());
        answer(
            reply,
            response(vec![tool(
                "cargo",
                "check-format",
                json!({"operation":"fmt_check"}),
            )]),
        );
        let (checked, reply) = actor.request().await;
        let result = latest_cargo(&checked);
        assert!(!result.is_error());
        assert_eq!(result.workspace_revision, Some(2));
        answer(
            reply,
            response(vec![text(
                "Fixed the answer; the focused feature-gated regression passes.",
            )]),
        );
        actor
            .event(|event| {
                matches!(
                    event.packet,
                    ActorToTuiPacket::TurnChanged {
                        state: Lifecycle::Completed,
                        ..
                    }
                )
            })
            .await;
        actor.stop().await;
    }
}
