use super::*;
use crate::{
    worker::{Worker, WorkerAdapter},
    workers::snapshot_worker::{Snapshot, SnapshotWorker},
};
use async_trait::async_trait;
use clients::llm::{self, StreamEvent, StreamProvider};
use common_models::knowledge::*;
use conversation::context::ContextLimits;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use ractor::{Actor, ActorProcessingErr};
use tokio::sync::oneshot;

type Events = BoxStream<'static, anyhow::Result<StreamEvent>>;

struct Captured {
    request: llm::ClientRequest,
    reply: oneshot::Sender<anyhow::Result<Events>>,
}

struct Provider(flume::Sender<Captured>);

impl StreamProvider for Provider {
    fn chat_stream(
        &self,
        request: llm::ClientRequest,
    ) -> BoxFuture<'static, anyhow::Result<Events>> {
        let sender = self.0.clone();
        Box::pin(async move {
            let (reply, receive) = oneshot::channel();
            sender.send_async(Captured { request, reply }).await?;
            receive.await?
        })
    }
}

struct Owner;

#[async_trait]
impl Worker for Owner {
    type Msg = Message;
    type State = ();
    type Arguments = ();
    async fn start(&self, _: ActorRef<Message>, _: ()) -> Result<(), ActorProcessingErr> {
        Ok(())
    }
    async fn handle(
        &self,
        _: ActorRef<Message>,
        _: Message,
        _: &mut (),
    ) -> Result<(), ActorProcessingErr> {
        Ok(())
    }
}

struct Fixture {
    directory: session::test_support::Workspace,
    workspace: Arc<WorkspacePolicy>,
    registry: Arc<ImmutableWorkerRegistry>,
    actor: ActorRef<Message>,
    handle: tokio::task::JoinHandle<()>,
    client: LLmClient,
    requests: flume::Receiver<Captured>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = session::test_support::Workspace::new();
        std::fs::write(directory.path.join("lib.rs"), "pub fn retained() {}\n").unwrap();
        let workspace = Arc::new(WorkspacePolicy::workspace(directory.path.clone()).unwrap());
        let (sender, requests) = flume::unbounded();
        let client = LLmClient::Injected(Arc::new(Provider(sender)));
        let (actor, handle) = Actor::spawn(None, WorkerAdapter::new(Owner), ())
            .await
            .unwrap();
        Self {
            directory,
            workspace,
            registry: Arc::default(),
            actor,
            handle,
            client,
            requests,
        }
    }

    fn context(&self, window: usize) -> BuildContext<'_> {
        BuildContext {
            workspace: self.workspace.clone(),
            client: &self.client,
            actor: &self.actor,
            budget: ContextBudget::Fixed(ContextLimits::new(window, 1024).unwrap()),
            timeout: Duration::from_secs(2),
        }
    }

    fn prepared(&self) -> PreparedKnowledge {
        let source = SourceFile::new(
            SourcePath::try_from("lib.rs".to_owned()).unwrap(),
            self.workspace.read(std::path::Path::new("lib.rs")).unwrap(),
        )
        .unwrap();
        let graph = SemanticGraph::try_from(GraphData {
            version: KNOWLEDGE_PROTOCOL_VERSION,
            profile: SemanticProfile {
                manifest: SourcePath::try_from("Cargo.toml".to_owned()).unwrap(),
                target: utils::knowledge::native_target(),
                features: Features::Default,
                configurations: BTreeSet::from([Configuration::Normal]),
                analyzer_version: ANALYZER_VERSION.into(),
            },
            sources: vec![source],
            symbols: Vec::new(),
            relations: Vec::new(),
            diagnostics: Vec::new(),
        })
        .unwrap();
        PreparedKnowledge {
            graph,
            fingerprint: Fingerprint::capture(&self.workspace).unwrap(),
        }
    }

    async fn build(&self, window: usize) -> GenerationSummary {
        let preparation = Preparation::begin(&self.registry, "owner").unwrap();
        self.registry
            .build_knowledge(&preparation, self.prepared(), self.context(window))
            .await
            .unwrap()
    }

    fn ask(
        &self,
        id: &str,
        question: &str,
    ) -> tokio::task::JoinHandle<anyhow::Result<crate::immutable_workers::ImmutableAnswer>> {
        let registry = self.registry.clone();
        let id = id.to_owned();
        let question = question.to_owned();
        tokio::spawn(async move {
            registry
                .ask("owner", &id, question, Duration::from_secs(3))
                .await
        })
    }

    async fn captured(&self) -> Captured {
        within(self.requests.recv_async()).await.unwrap()
    }

    async fn stop(self) {
        self.registry.clear("owner").await;
        self.actor.stop(None);
        within(self.handle).await.unwrap();
    }
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .unwrap()
}

fn answer(reply: oneshot::Sender<anyhow::Result<Events>>, text: &str) {
    let events = vec![
        StreamEvent::MessageStart {
            message: llm::StreamMessage {
                id: "fixture".into(),
                model: "fixture".into(),
                role: llm::Role::Assistant,
                usage: Default::default(),
            },
        },
        StreamEvent::ContentBlockComplete {
            index: 0,
            content: llm::ContentBlock::MessageBlock {
                text: text.into(),
                phase: None,
            },
        },
        StreamEvent::MessageDelta {
            delta: llm::MessageDeltaContent {
                stop_reason: Some(llm::StopReason::EndTurn),
            },
            usage: Default::default(),
        },
        StreamEvent::MessageStop,
    ];
    assert!(
        reply
            .send(Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed()))
            .is_ok()
    );
}

#[tokio::test]
async fn knowledge_workers_use_measured_tool_free_independent_requests() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(4096).await;
    assert!(fixture.requests.is_empty());
    assert!(fixture.registry.list("another-conversation").is_empty());
    let generation = fixture.registry.knowledge("owner").unwrap();
    let worker = &summary.workers[0].route.worker.worker_id;
    let mut first = None;
    for question in ["What is retained?", "What evidence is missing?"] {
        let ask = fixture.ask(worker, question);
        let captured = fixture.captured().await;
        assert!(captured.request.tools.is_empty());
        assert_eq!(captured.request.messages.len(), 2);
        assert_eq!(captured.request.messages[1].text(), question);
        assert_eq!(captured.request.max_output_tokens, Some(1024));
        assert!(
            generation
                .index
                .budget
                .admits(estimated_tokens(&captured.request).unwrap())
        );
        let mut without_question = captured.request.clone();
        without_question.messages.pop();
        assert_eq!(
            estimated_tokens(&without_question).unwrap(),
            generation.index.shards[0].summary.estimated_tokens
        );
        match &first {
            Some(first) => assert_eq!(first, &captured.request.messages[0].text()),
            None => first = Some(captured.request.messages[0].text()),
        }
        answer(captured.reply, "lib.rs:1 contains retained");
        assert_eq!(
            within(ask).await.unwrap().unwrap().answer,
            "lib.rs:1 contains retained"
        );
    }
    assert!(
        generation
            .search("lib.rs", Some("wrong-generation"), 0, 10)
            .is_err()
    );
    assert!(generation.search("lib.rs", None, 1, 10).is_err());
    let route = generation.search("lib.rs", None, 0, 10).unwrap();
    assert_eq!(route.workers[0].worker.worker_id, *worker);
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_detects_source_changes_before_and_after_answering() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(8192).await;
    let ask = fixture.ask(
        &summary.workers[0].route.worker.worker_id,
        "What is retained?",
    );
    let captured = fixture.captured().await;
    std::fs::write(
        fixture.directory.path.join("lib.rs"),
        "pub fn changed() {}\n",
    )
    .unwrap();
    answer(captured.reply, "old source");
    assert!(within(ask).await.unwrap().is_err());
    assert!(
        fixture
            .registry
            .knowledge("owner")
            .unwrap()
            .freshness
            .cancel
            .is_cancelled()
    );
    assert!(
        within(fixture.ask(&summary.workers[0].route.worker.worker_id, "Again?"))
            .await
            .unwrap()
            .is_err()
    );
    assert!(fixture.requests.is_empty());
    assert!(
        fixture
            .registry
            .repartition_knowledge("owner", fixture.context(8192))
            .await
            .is_err()
    );
    assert!(matches!(
        fixture
            .registry
            .knowledge_status("owner", &fixture.workspace, summary.budget, 0, 10)
            .await
            .unwrap(),
        Status::Stale { .. }
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_publication_rollback_repartition_and_targeted_clear_preserve_snapshots() {
    let fixture = Fixture::new().await;
    let first = fixture.build(8192).await;
    let snapshot = Snapshot::new(
        llm::ClientRequest::new(vec![llm::Message::new("Historical context".into())]),
        &fixture.client,
        ContextLimits::new(8192, 1024).unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    let snapshot = ImmutableWorker::spawn(
        SnapshotWorker,
        snapshot,
        ImmutableWorkerDescription {
            kind: "snapshot".into(),
            description: "historical".into(),
        },
        &fixture.actor,
    )
    .await
    .unwrap();
    let snapshot = fixture.registry.insert("owner", snapshot);
    let failed = Preparation::begin(&fixture.registry, "owner").unwrap();
    assert!(Preparation::begin(&fixture.registry, "owner").is_err());
    assert_eq!(
        fixture
            .registry
            .knowledge("owner")
            .unwrap()
            .index
            .generation,
        first.generation
    );
    drop(failed);
    assert_eq!(
        fixture
            .registry
            .knowledge("owner")
            .unwrap()
            .index
            .generation,
        first.generation
    );
    let previous = fixture.registry.knowledge("owner").unwrap();
    let second = fixture
        .registry
        .repartition_knowledge("owner", fixture.context(4096))
        .await
        .unwrap();
    assert_ne!(first.generation, second.generation);
    assert_eq!(second.budget.window(), 4096);
    assert!(previous.freshness.cancel.is_cancelled());
    assert!(fixture.requests.is_empty());
    assert_eq!(fixture.registry.list("owner").len(), 2);
    assert!(
        fixture
            .registry
            .ask(
                "owner",
                &first.workers[0].route.worker.worker_id,
                "old?".into(),
                Duration::from_secs(1)
            )
            .await
            .is_err()
    );
    fixture.registry.clear_knowledge("owner");
    assert!(fixture.registry.knowledge("owner").is_err());
    assert_eq!(
        fixture.registry.list("owner")[0].worker_id,
        snapshot.worker_id
    );
    let pending = Preparation::begin(&fixture.registry, "owner").unwrap();
    fixture.registry.clear("owner").await;
    assert!(
        fixture
            .registry
            .build_knowledge(&pending, fixture.prepared(), fixture.context(8192))
            .await
            .is_err()
    );
    assert!(fixture.registry.list("owner").is_empty());
    drop(pending);
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_caller_cancellation_and_retirement_drop_provider_requests() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(8192).await;
    let worker = &summary.workers[0].route.worker.worker_id;
    let ask = fixture.ask(worker, "Cancel this question");
    let mut captured = fixture.captured().await;
    ask.abort();
    assert!(within(ask).await.is_err());
    within(captured.reply.closed()).await;
    let ask = fixture.ask(worker, "Retire this generation");
    let mut captured = fixture.captured().await;
    fixture.registry.clear_knowledge("owner");
    within(captured.reply.closed()).await;
    assert!(within(ask).await.unwrap().is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_budget_scope_and_model_mismatches_fail_closed() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(8192).await;
    let generation = fixture.registry.knowledge("owner").unwrap();
    let other = session::test_support::Workspace::new();
    let other = WorkspacePolicy::workspace(other.path.clone()).unwrap();
    assert!(generation.check(&other, summary.budget).await.is_err());
    assert!(
        fixture
            .registry
            .knowledge_status("owner", &other, summary.budget, 0, 10)
            .await
            .is_err()
    );
    assert!(
        generation
            .check(
                &fixture.workspace,
                KnowledgeBudget::new(4096, 4096, 1024).unwrap()
            )
            .await
            .is_err()
    );
    let mut freshness = Freshness::new(
        Fingerprint::capture(&fixture.workspace).unwrap(),
        fixture.workspace.clone(),
        &fixture.client,
    );
    freshness.model.window += 1;
    assert!(Arc::new(freshness).check().await.is_err());
    let huge_question = "different token \" λ ∑ 🚀 ".repeat(800);
    assert!(
        within(fixture.ask(&summary.workers[0].route.worker.worker_id, &huge_question))
            .await
            .unwrap()
            .is_err()
    );
    assert!(fixture.requests.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_shards_share_provider_capacity_and_cancelled_mail_remains_bounded() {
    let fixture = Fixture::new().await;
    std::fs::write(
        fixture.directory.path.join("lib.rs"),
        "pub fn retained() {}\n".repeat(800),
    )
    .unwrap();
    let summary = fixture.build(4096).await;
    assert!(summary.shards > 1);
    let worker = &summary.workers[0].route.worker.worker_id;
    let first = fixture.ask(worker, "first shard");
    let captured = fixture.captured().await;
    let second = fixture.ask(&summary.workers[1].route.worker.worker_id, "second shard");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), fixture.requests.recv_async())
            .await
            .is_err()
    );
    answer(captured.reply, "first");
    assert!(within(first).await.unwrap().is_ok());
    answer(fixture.captured().await.reply, "second");
    assert!(within(second).await.unwrap().is_ok());

    let active = fixture.ask(worker, "hold the mailbox");
    let captured = fixture.captured().await;
    let queued: Vec<_> = (0..15).map(|_| fixture.ask(worker, "queued")).collect();
    within(async {
        while fixture.registry.knowledge_queue.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    for task in queued {
        task.abort();
        assert!(within(task).await.is_err());
    }
    assert_eq!(fixture.registry.knowledge_queue.available_permits(), 0);
    assert!(
        within(fixture.ask(worker, "over capacity"))
            .await
            .unwrap()
            .is_err()
    );
    answer(captured.reply, "done");
    assert!(within(active).await.unwrap().is_ok());
    within(async {
        while fixture.registry.knowledge_queue.available_permits() != 16 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(fixture.requests.is_empty());
    fixture.stop().await;
}

#[test]
fn knowledge_tool_classifies_preparation_as_validation_and_queries_as_read_only() {
    use crate::{
        actor::ActorContext,
        tools::knowledge::{Input, Knowledge},
    };
    use analysis::contexts::rust_context::RustContext;
    use tools::tool_defs::{LenientDeserialize, ToolOpKind, erased_tool};
    let tool = erased_tool::<Knowledge, RustContext, ActorContext<RustContext>>();
    for action in ["status", "search", "inspect", "clear", "repartition"] {
        let value = match action {
            "search" => serde_json::json!({"action":action,"query":"symbol"}),
            "inspect" => serde_json::json!({"action":action,"symbol":"id","generation":"id"}),
            _ => serde_json::json!({"action":action}),
        };
        assert_eq!(
            tool.effect_from_input_erased(&value).unwrap(),
            ToolOpKind::Read
        );
    }
    assert_eq!(
        tool.effect_from_input_erased(&serde_json::json!({"action":"prepare"}))
            .unwrap(),
        ToolOpKind::Validate
    );
    assert!(
        Input::deserialize_lenient(serde_json::json!({"action":"prepare","query":"unused"}))
            .is_err()
    );
    assert!(Input::deserialize_lenient(serde_json::json!({"action":"search"})).is_err());
    assert!(
        Input::deserialize_lenient(serde_json::json!({"action":"status","features":null})).is_ok()
    );
    assert!(crate::tools::knowledge::access::<RustContext>(&ActorContext::Noop).is_err());
}

#[test]
fn knowledge_validity_keeps_the_first_invalidation_reason() {
    let mut validity = Validity::Current;
    assert!(validity.current().is_ok());
    validity.invalidate("first reason");
    validity.invalidate("second reason");
    assert_eq!(
        validity.current().unwrap_err().to_string(),
        "Knowledge generation is stale: first reason"
    );
}

#[tokio::test]
async fn knowledge_status_validates_pages_even_when_no_generation_exists() {
    let fixture = Fixture::new().await;
    let budget = KnowledgeBudget::new(8192, 8192, 1024).unwrap();
    for limit in [0, 33, usize::MAX] {
        assert!(
            fixture
                .registry
                .knowledge_status("owner", &fixture.workspace, budget, 0, limit)
                .await
                .is_err()
        );
    }
    assert!(
        fixture
            .registry
            .knowledge_status("owner", &fixture.workspace, budget, 257, 10)
            .await
            .is_err()
    );
    assert!(matches!(
        fixture
            .registry
            .knowledge_status("owner", &fixture.workspace, budget, 256, 32)
            .await
            .unwrap(),
        Status::Absent
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_rollback_never_resurrects_cleared_or_superseded_preparations() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(8192).await;
    let abandoned = Preparation::begin(&fixture.registry, "owner").unwrap();
    fixture.registry.clear_knowledge("owner");
    assert!(abandoned.cancel.is_cancelled());
    let replacement = Preparation::begin(&fixture.registry, "owner").unwrap();
    drop(abandoned);
    let status = fixture
        .registry
        .knowledge_status("owner", &fixture.workspace, summary.budget, 0, 10)
        .await
        .unwrap();
    assert!(matches!(status, Status::Preparing { ticket } if ticket == replacement.ticket));
    assert!(!replacement.cancel.is_cancelled());
    drop(replacement);
    assert!(matches!(
        fixture
            .registry
            .knowledge_status("owner", &fixture.workspace, summary.budget, 0, 10)
            .await
            .unwrap(),
        Status::Absent
    ));
    assert!(fixture.registry.list("owner").is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_cancelled_publication_rolls_back_without_retiring_previous_workers() {
    let fixture = Fixture::new().await;
    let summary = fixture.build(8192).await;
    let previous = fixture.registry.knowledge("owner").unwrap();
    let pending = Preparation::begin(&fixture.registry, "owner").unwrap();
    pending.cancel.cancel();
    assert!(pending.publish(previous.clone(), Vec::new()).is_err());
    drop(pending);
    assert!(!previous.freshness.cancel.is_cancelled());
    assert!(Arc::ptr_eq(
        &previous,
        &fixture.registry.knowledge("owner").unwrap()
    ));
    assert_eq!(
        fixture.registry.list("owner")[0].worker_id,
        summary.workers[0].route.worker.worker_id
    );
    assert!(matches!(
        fixture
            .registry
            .knowledge_status("owner", &fixture.workspace, summary.budget, 0, 1)
            .await
            .unwrap(),
        Status::Ready { .. }
    ));
    fixture.stop().await;
}
