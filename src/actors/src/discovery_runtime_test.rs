use super::*;
use crate::workers::{base_worker::BaseWorker, simple_worker::SimpleWorker};
use analysis::contexts::rust_context::RustContext;

#[derive(Clone, Copy)]
enum Mode {
    Simple,
    Delegated,
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
                debug_mode: false,
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
