use super::*;
use crate::immutable_workers::{ImmutableWorker, ImmutableWorkerDescription, ImmutableWorkerView};
use crate::workers::snapshot_worker::Snapshot;
use conversation::context::ContextLimits;

struct SnapshotHarness {
    owner: Harness,
    worker: ImmutableWorkerView,
    requests: flume::Receiver<Request>,
}

impl SnapshotHarness {
    async fn new(request: llm::ClientRequest, timeout: Duration) -> Self {
        let (tx, requests) = flume::unbounded();
        let client = llm::LLmClient::Injected(Arc::new(Provider(tx)));
        let snapshot = Snapshot::new(
            request,
            &client,
            ContextLimits::new(16_000, 2048).unwrap(),
            timeout,
        )
        .unwrap();
        Self::spawn(snapshot, requests).await
    }

    async fn spawn(snapshot: Snapshot, requests: flume::Receiver<Request>) -> Self {
        let owner = Harness::new(vec![], Duration::from_secs(1)).await;
        let worker = ImmutableWorker::snapshot(
            snapshot,
            ImmutableWorkerDescription {
                kind: "snapshot".into(),
                description: "Frozen fixture context".into(),
            },
            &owner.actor,
        );
        let worker = owner.runtime.immutable_workers.insert("actor-1", worker);
        Self {
            owner,
            worker,
            requests,
        }
    }

    fn ask(&self, question: &str) -> oneshot::Receiver<anyhow::Result<String>> {
        let (reply, receive) = oneshot::channel();
        let registry = self.owner.runtime.immutable_workers.clone();
        let worker = self.worker.worker_id.clone();
        let question = question.to_owned();
        tokio::spawn(async move {
            let result = registry
                .ask("actor-1", &worker, question, Duration::from_secs(2))
                .await
                .map(|answer| answer.answer);
            let _ = reply.send(result);
        });
        receive
    }

    async fn request(&self) -> Request {
        within(self.requests.recv_async()).await.unwrap()
    }

    async fn stop(self) {
        self.owner.stop().await;
    }
}

fn context() -> llm::ClientRequest {
    llm::ClientRequest::new(vec![llm::Message::new("Captured workspace context".into())])
        .with_system("Captured instructions".into())
}

fn frozen(request: &llm::ClientRequest) -> Value {
    json!({
        "messages": &request.messages[..request.messages.len() - 2],
        "tools": request.tools,
    })
}

async fn capture(actor: &ActorRef<Message>) -> anyhow::Result<Snapshot> {
    let (reply, receive) = oneshot::channel();
    actor
        .send_message(Message::CaptureSnapshot(reply.into()))
        .unwrap();
    within(receive).await.unwrap()
}

async fn register_snapshot(h: &Harness, owner: &str) -> ImmutableWorkerView {
    let worker = ImmutableWorker::snapshot(
        capture(&h.actor).await.unwrap(),
        ImmutableWorkerDescription {
            kind: "snapshot".into(),
            description: "Frozen fixture context".into(),
        },
        &h.actor,
    );
    h.runtime.immutable_workers.insert(owner, worker)
}

#[tokio::test]
async fn listed_question_allowance_fits_through_the_regular_worker() {
    let source = context();
    let expected = serde_json::to_value(&source.messages).unwrap();
    let h = SnapshotHarness::new(source, Duration::from_secs(1)).await;
    let allowance = h.worker.max_question_bytes.unwrap();
    assert_eq!(
        h.owner.runtime.immutable_workers.list("actor-1")[0].max_question_bytes,
        Some(allowance)
    );
    let question = "\0".repeat(allowance);
    let result = h.ask(&question);
    let (request, reply) = h.request().await;
    assert_eq!(frozen(&request)["messages"], expected);
    assert_eq!(request.messages.last().unwrap().text(), question);
    assert!(
        conversation::context::estimated_tokens(&request).unwrap()
            <= ContextLimits::new(16_000, 2048).unwrap().input()
    );
    answer(reply, response(vec![text("The entire question fits")]));
    assert_eq!(
        within(result).await.unwrap().unwrap(),
        "The entire question fits"
    );
    h.stop().await;
}

#[tokio::test]
async fn registered_snapshot_survives_completed_and_interrupted_owner_turns() {
    enum OwnerTurn {
        Complete,
        Interrupt,
    }

    let h = Harness::new(vec![], Duration::from_secs(1)).await;
    let owner = "actor-1";
    let view = register_snapshot(&h, owner).await;
    let registry = h.runtime.immutable_workers.clone();
    let mut prefix = None;
    for turn in [OwnerTurn::Complete, OwnerTurn::Interrupt] {
        h.start("Later owner context must not change the snapshot");
        let (_, reply) = h.request().await;
        match turn {
            OwnerTurn::Complete => {
                answer(reply, response(vec![text("Later owner answer")]));
                h.terminal(Lifecycle::Completed).await;
            }
            OwnerTurn::Interrupt => {
                h.actor.send_message(Message::Interrupt).unwrap();
                h.terminal(Lifecycle::Cancelled).await;
                assert!(reply.is_closed());
            }
        }
        assert_eq!(registry.list(owner).len(), 1);
        let serve = async {
            let (request, reply) = h.request().await;
            assert_eq!(request.messages.len(), 4);
            assert!(!request.tools.is_empty());
            let current =
                serde_json::to_string(&request.messages[..request.messages.len() - 2]).unwrap();
            assert!(!current.contains("Later owner"));
            match &prefix {
                Some(previous) => assert_eq!(previous, &current),
                None => prefix = Some(current),
            }
            answer(reply, response(vec![text("Independent snapshot answer")]));
        };
        let ask = registry.ask(
            owner,
            &view.worker_id,
            "What was captured?".into(),
            Duration::from_secs(2),
        );
        let (result, ()) = within(async { tokio::join!(ask, serve) }).await;
        assert_eq!(result.unwrap().answer, "Independent snapshot answer");
    }
    h.stop().await;
    assert!(registry.list(owner).is_empty());
}

#[tokio::test]
async fn clearing_registered_snapshots_cancels_pending_requests_and_streams() {
    enum Pending {
        Request,
        Stream,
    }

    for pending in [Pending::Request, Pending::Stream] {
        let h = Harness::new(vec![], Duration::from_secs(1)).await;
        let owner = "actor-1";
        let view = register_snapshot(&h, owner).await;
        let registry = &h.runtime.immutable_workers;
        let clear = async {
            let (_, reply) = h.request().await;
            match pending {
                Pending::Request => {
                    registry.clear(owner).await;
                    assert!(reply.is_closed());
                }
                Pending::Stream => {
                    let (events, stream) = flume::unbounded::<StreamEvent>();
                    assert!(reply.send(Ok(stream.into_stream().map(Ok).boxed())).is_ok());
                    registry.clear(owner).await;
                    assert!(events.is_disconnected());
                }
            }
        };
        let ask = registry.ask(
            owner,
            &view.worker_id,
            "Wait for an answer".into(),
            Duration::from_secs(2),
        );
        let (result, ()) = within(async { tokio::join!(ask, clear) }).await;
        assert!(result.is_err());
        assert!(registry.list(owner).is_empty());
        assert!(h.requests.is_empty());
        h.stop().await;
    }
}

#[tokio::test]
async fn repeated_questions_preserve_context_without_retaining_questions_or_answers() {
    let mut source = context().with_prompt_cache_key(Some("parent".into()));
    let expected = serde_json::to_value(&source.messages).unwrap();
    let h = SnapshotHarness::new(source.clone(), Duration::from_secs(1)).await;
    source
        .messages
        .push(llm::Message::new("Later source change".into()));
    source.system = Some("Changed source instructions".into());
    let mut prefix = None;
    let mut key = None;
    for question in [
        "First independent question",
        "Second independent question",
        "First independent question",
    ] {
        let result = h.ask(question);
        let (request, reply) = h.request().await;
        assert_eq!(request.messages.len(), 3);
        assert!(
            request.messages[1]
                .text()
                .starts_with("Answer only the question")
        );
        assert_eq!(request.messages[2].text(), question);
        assert_eq!(frozen(&request)["messages"], expected);
        assert!(
            request
                .system
                .as_deref()
                .unwrap()
                .starts_with("Captured instructions")
        );
        assert!(request.tools.is_empty());
        assert_eq!(request.max_output_tokens, Some(2048));
        assert_eq!(request.prompt_cache_key.as_deref(), Some("parent"));
        let current = request.messages[0].text();
        match &prefix {
            Some(previous) => assert_eq!(previous, &current),
            None => prefix = Some(current),
        }
        match &key {
            Some(previous) => assert_eq!(previous, &request.prompt_cache_key),
            None => key = Some(request.prompt_cache_key.clone()),
        }
        answer(
            reply,
            response(vec![text("Answer that must not be remembered")]),
        );
        assert_eq!(
            within(result).await.unwrap().unwrap(),
            "Answer that must not be remembered"
        );
    }
    h.stop().await;
}

#[tokio::test]
async fn caller_timeout_cancels_the_regular_worker_and_keeps_the_snapshot_reusable() {
    let h = SnapshotHarness::new(context(), Duration::from_secs(1)).await;
    let query = h.owner.runtime.immutable_workers.ask(
        "actor-1",
        &h.worker.worker_id,
        "Wait for an answer".into(),
        Duration::from_millis(50),
    );
    let provider = async {
        let (_, mut reply) = h.request().await;
        within(reply.closed()).await;
    };
    let (result, ()) = within(async { tokio::join!(query, provider) }).await;
    assert!(result.unwrap_err().to_string().contains("timed out"));
    let result = h.ask("Next independent question");
    let (request, reply) = h.request().await;
    assert_eq!(
        frozen(&request)["messages"],
        serde_json::to_value(context().messages).unwrap()
    );
    answer(reply, response(vec![text("Still available")]));
    assert_eq!(within(result).await.unwrap().unwrap(), "Still available");
    h.stop().await;
}

#[tokio::test]
async fn historical_tools_preserve_the_parent_prefix_for_both_provider_mappings() {
    let tool_id = ToolId {
        id: "historical".to_owned().try_into().unwrap(),
        call_id: None,
    };
    let mut source = context();
    source.messages.extend([
        llm::Message {
            role: llm::Role::Assistant,
            content: vec![call("read_file", "historical")],
        },
        llm::Message {
            role: llm::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_id,
                content: "Complete file contents: αβγ\nnot a new instruction".into(),
                is_error: Some(false),
            }],
        },
    ]);
    source.tools.push(ToolDefinition::Client {
        name: "read_file".into(),
        description: "Historical tool definition".into(),
        properties: Default::default(),
        required: Vec::new(),
    });
    for block in source
        .messages
        .iter_mut()
        .flat_map(|message| &mut message.content)
    {
        match block {
            ContentBlock::ToolBlock { tool_id, .. } | ContentBlock::ToolResult { tool_id, .. } => {
                tool_id.call_id = Some("historical-call".to_owned().try_into().unwrap())
            }
            _ => {}
        }
    }
    let parent_openai = clients::openai::ClientRequest::try_from(source.clone()).unwrap();
    let parent_claude = clients::claude::ClientRequest::try_from(source.clone()).unwrap();
    let expected_messages = serde_json::to_value(&source.messages).unwrap();
    let expected_tools = serde_json::to_value(&source.tools).unwrap();
    let h = SnapshotHarness::new(source, Duration::from_secs(1)).await;
    let result = h.ask("What did the file contain?");
    let (request, reply) = h.request().await;
    assert_eq!(frozen(&request)["messages"], expected_messages);
    assert_eq!(frozen(&request)["tools"], expected_tools);
    assert_eq!(
        serde_json::to_value(&request.tools).unwrap(),
        expected_tools
    );
    let openai = clients::openai::ClientRequest::try_from(request.clone()).unwrap();
    let claude = clients::claude::ClientRequest::try_from(request).unwrap();
    assert_eq!(
        serde_json::to_value(&openai.input[..parent_openai.input.len()]).unwrap(),
        serde_json::to_value(&parent_openai.input).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&openai.tools).unwrap(),
        serde_json::to_value(&parent_openai.tools).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&claude.messages[..parent_claude.messages.len()]).unwrap(),
        serde_json::to_value(&parent_claude.messages).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&claude.tools).unwrap(),
        serde_json::to_value(&parent_claude.tools).unwrap()
    );
    answer(reply, response(vec![text("The file contained αβγ")]));
    assert_eq!(
        within(result).await.unwrap().unwrap(),
        "The file contained αβγ"
    );
    h.stop().await;
}

#[tokio::test]
async fn incomplete_history_is_rejected_even_in_the_first_message() {
    let (tx, requests) = flume::unbounded();
    let client = llm::LLmClient::Injected(Arc::new(Provider(tx)));
    for messages in [
        vec![llm::Message {
            role: llm::Role::Assistant,
            content: vec![call("read_file", "pending")],
        }],
        vec![
            llm::Message::new("workspace".into()),
            llm::Message {
                role: llm::Role::Assistant,
                content: vec![call("read_file", "pending")],
            },
        ],
    ] {
        assert!(
            Snapshot::new(
                llm::ClientRequest::new(messages),
                &client,
                ContextLimits::new(16_000, 2048).unwrap(),
                Duration::from_secs(1),
            )
            .is_err()
        );
    }
    assert!(requests.is_empty());
}

#[tokio::test]
async fn capture_rejects_active_turns_and_survives_source_changes_and_shutdown() {
    let h = Harness::new(vec![], Duration::from_secs(1)).await;
    h.start("Remember the original requirement");
    let (source_request, reply) = h.request().await;
    assert!(
        capture(&h.actor)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("Finish the active turn")
    );
    answer(reply, response(vec![text("Original source answer")]));
    h.terminal(Lifecycle::Completed).await;
    let history = h.history().await;
    let snapshot = capture(&h.actor).await.unwrap();
    assert_eq!(
        serde_json::to_value(h.history().await).unwrap(),
        serde_json::to_value(&history).unwrap()
    );
    h.start("Later requirement not part of the snapshot");
    let (_, reply) = h.request().await;
    answer(reply, response(vec![text("Later answer")]));
    h.terminal(Lifecycle::Completed).await;
    let requests = h.requests.clone();
    h.stop().await;
    let snapshot = SnapshotHarness::spawn(snapshot, requests).await;
    let result = snapshot.ask("What was the original requirement?");
    let (request, reply) = snapshot.request().await;
    let captured: Vec<llm::Message> =
        serde_json::from_value(frozen(&request)["messages"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&captured[..history.len()]).unwrap(),
        serde_json::to_value(&history).unwrap()
    );
    assert_eq!(captured.len(), history.len());
    assert_eq!(
        runtime_snapshot(&captured),
        runtime_snapshot(&source_request.messages)
    );
    assert_eq!(request.prompt_cache_key, source_request.prompt_cache_key);
    assert!(
        request
            .system
            .unwrap()
            .contains("Follow the fixture's operating instructions.")
    );
    answer(
        reply,
        response(vec![text("Remember the original requirement")]),
    );
    assert_eq!(
        within(result).await.unwrap().unwrap(),
        "Remember the original requirement"
    );
    snapshot.stop().await;
}

#[tokio::test]
async fn capture_matches_parent_active_context_after_compaction() {
    let h = Harness::new(vec![], Duration::from_secs(1)).await;
    let (tx, requests) = flume::unbounded();
    let (tui_tx, _) = flume::unbounded();
    let mut state = ActorState::new(
        Dependency {
            client: llm::LLmClient::Injected(Arc::new(Provider(tx))),
            tools: vec![],
            tui_tx,
            debug_mode: false,
            context: TestContext {
                task: None,
                revision: 7,
            },
            runtime: Runtime::default(),
        },
        h.actor.clone(),
        None,
    )
    .await
    .unwrap();
    state.session.conversation.append([
        llm::Message::new("Old exact requirement not included in the summary".into()),
        llm::Message::new_assistant("Old detailed evidence".into()),
        llm::Message::new("Recent question".into()),
        llm::Message::new_assistant("Recent answer".into()),
    ]);
    state.session.conversation.commit_checkpoint(
        conversation::context::Checkpoint::new(
            state.session.conversation.history(),
            3,
            1,
            conversation::context::Memory::Summary("Existing summary".into()),
        )
        .unwrap(),
    );
    let input = state
        .provider_context()
        .input(common_models::runtime_ids::TurnId::new(), &state.llm)
        .unwrap();
    let parent = input.request(&input.checkpoint).unwrap();
    let expected = serde_json::to_value(&parent.messages).unwrap();
    let snapshot = state
        .provider_context()
        .capture_snapshot(&state.turn, &state.llm)
        .unwrap();
    state.session.conversation = conversation::Conversation::new(Vec::new(), None);
    state.context.revision = 99;
    let snapshot = SnapshotHarness::spawn(snapshot, requests).await;
    let result = snapshot.ask("Recall the old exact requirement");
    let (request, reply) = snapshot.request().await;
    let captured: Vec<llm::Message> =
        serde_json::from_value(frozen(&request)["messages"].clone()).unwrap();
    assert_eq!(serde_json::to_value(&captured).unwrap(), expected);
    assert_eq!(request.system, parent.system);
    assert_eq!(request.prompt_cache_key, parent.prompt_cache_key);
    assert_eq!(
        serde_json::to_value(&request.tools).unwrap(),
        serde_json::to_value(&parent.tools).unwrap()
    );
    assert!(
        captured
            .iter()
            .any(|message| message.text().contains("Existing summary"))
    );
    assert!(
        !captured
            .iter()
            .any(|message| message.text().contains("Old detailed evidence"))
    );
    assert_eq!(captured[0].text(), "workspace revision 7");
    answer(
        reply,
        response(vec![text(
            "Old exact requirement not included in the summary",
        )]),
    );
    assert_eq!(
        within(result).await.unwrap().unwrap(),
        "Old exact requirement not included in the summary"
    );
    snapshot.stop().await;
    h.stop().await;
}

#[tokio::test]
async fn invalid_questions_do_not_call_provider_or_poison_actor() {
    let h = SnapshotHarness::new(context(), Duration::from_secs(1)).await;
    for question in [
        String::new(),
        " \n ".into(),
        "oversized question ".repeat(16_000),
    ] {
        assert!(within(h.ask(&question)).await.unwrap().is_err());
        assert!(h.requests.is_empty());
    }
    let result = h.ask("A valid question");
    let (request, reply) = h.request().await;
    assert_eq!(request.messages.len(), 3);
    answer(reply, response(vec![text("Still available")]));
    assert_eq!(within(result).await.unwrap().unwrap(), "Still available");
    h.stop().await;
}

#[tokio::test]
async fn malformed_tool_refused_and_empty_answers_leave_snapshot_reusable() {
    let h = SnapshotHarness::new(context(), Duration::from_secs(1)).await;
    let mut incomplete = response(vec![text("Partial answer")]);
    incomplete.pop();
    let mut trailing = response(vec![text("Already complete")]);
    trailing.push(StreamEvent::ContentBlockComplete {
        index: 1,
        content: text("Late content"),
    });
    let mut failures = vec![
        incomplete,
        trailing,
        response(vec![text(" \n ")]),
        response(vec![ContentBlock::ThinkingBlock {
            thinking: "No visible answer".into(),
            signature: "signature".into(),
            reasoning_id: None,
        }]),
        response(vec![call("apply_patch", "forbidden")]),
    ];
    for reason in [
        llm::StopReason::Refusal,
        llm::StopReason::MaxTokens,
        llm::StopReason::ContextExceeded,
    ] {
        let mut events = response(vec![text("Not a successful answer")]);
        if let Some(StreamEvent::MessageDelta { delta, .. }) = events.last_mut() {
            delta.stop_reason = Some(reason);
        }
        failures.push(events);
    }
    for events in failures {
        let result = h.ask("Answer from the original context");
        let (request, reply) = h.request().await;
        assert_eq!(request.messages.len(), 3);
        assert_eq!(
            frozen(&request)["messages"],
            serde_json::to_value(context().messages).unwrap()
        );
        answer(reply, events);
        assert!(within(result).await.unwrap().is_err());
        assert!(h.requests.is_empty());
    }
    let result = h.ask("Try a valid answer");
    let (_, reply) = h.request().await;
    answer(reply, response(vec![text("Valid answer")]));
    assert_eq!(within(result).await.unwrap().unwrap(), "Valid answer");
    h.stop().await;
}

#[tokio::test]
async fn provider_errors_and_stream_timeouts_do_not_retain_partial_answers() {
    let h = SnapshotHarness::new(context(), Duration::from_millis(100)).await;
    let result = h.ask("Provider failure");
    let (_, reply) = h.request().await;
    assert!(
        reply
            .send(Err(anyhow::anyhow!("Provider unavailable")))
            .is_ok()
    );
    assert!(
        within(result)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("Provider unavailable")
    );

    let result = h.ask("Transport failure during the stream");
    let (_, reply) = h.request().await;
    let mut partial = response(vec![text("Discard this partial answer")]);
    partial.pop();
    let stream = futures::stream::iter(partial.into_iter().map(Ok))
        .chain(futures::stream::once(async {
            Err(anyhow::anyhow!("Transport disconnected"))
        }))
        .boxed();
    assert!(reply.send(Ok(stream)).is_ok());
    assert!(
        within(result)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("Transport disconnected")
    );

    let result = h.ask("Timeout before the stream");
    let (_, reply) = h.request().await;
    assert!(
        within(result)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(reply.is_closed());

    let result = h.ask("Timeout during the stream");
    let (_, reply) = h.request().await;
    let mut partial = response(vec![text("Unfinished answer")]);
    partial.pop();
    let stream = futures::stream::iter(partial.into_iter().map(Ok))
        .chain(futures::stream::pending())
        .boxed();
    assert!(reply.send(Ok(stream)).is_ok());
    assert!(
        within(result)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );

    let result = h.ask("Next independent question");
    let (request, reply) = h.request().await;
    assert_eq!(request.messages.len(), 3);
    assert_eq!(
        frozen(&request)["messages"],
        serde_json::to_value(context().messages).unwrap()
    );
    answer(reply, response(vec![text("Recovered answer")]));
    assert_eq!(within(result).await.unwrap().unwrap(), "Recovered answer");
    h.stop().await;
}
