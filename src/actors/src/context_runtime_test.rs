use super::*;
use crate::{
    context::{ContextBudget, ContextLimits, estimated_tokens},
    session::SessionStore,
};
use commands::command::{Command, ResumeTarget};

struct NativeRequest {
    request: llm::ClientRequest,
    reply: oneshot::Sender<anyhow::Result<clients::compaction::CompactionResponse>>,
}

struct NativeProvider {
    normal: Provider,
    compactions: flume::Sender<NativeRequest>,
}

impl StreamProvider for NativeProvider {
    fn native_compaction(&self) -> bool {
        true
    }

    fn chat_stream(
        &self,
        request: llm::ClientRequest,
    ) -> BoxFuture<'static, anyhow::Result<Events>> {
        self.normal.chat_stream(request)
    }

    fn compact(
        &self,
        request: llm::ClientRequest,
    ) -> BoxFuture<'static, anyhow::Result<clients::compaction::CompactionResponse>> {
        let tx = self.compactions.clone();
        Box::pin(async move {
            let (reply, receive) = oneshot::channel();
            tx.send_async(NativeRequest { request, reply }).await?;
            receive.await?
        })
    }
}

fn configured_runtime(workspace: &crate::session::tests::Workspace) -> Runtime {
    Runtime {
        context_budget: ContextBudget::Fixed(configured_limits()),
        ..Runtime::for_workspace(workspace.path.clone()).unwrap()
    }
}

fn configured_limits() -> ContextLimits {
    ContextLimits::new(24_000, 2048).unwrap()
}

fn saved_history(store: &Arc<SessionStore>) -> String {
    let mut messages = vec![
        llm::Message::new("obsolete workspace context".into()),
        llm::Message::new(
            "Keep public APIs unchanged; preserve existing edits; finish the bug fix.".into(),
        ),
    ];
    for index in 0..5 {
        messages.push(llm::Message::new_assistant(format!(
            "Investigation {index}: {}",
            "inspected source ".repeat(2000)
        )));
        messages.push(llm::Message::new(format!("Continue requirement {index}")));
    }
    let session = store
        .create(llm::SessionProvider::Injected, None, messages)
        .unwrap();
    session.id.clone()
}

async fn resume(h: &Harness, id: &str) {
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.into(),
        })))
        .unwrap();
    let event = h
        .event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(_)))
        .await;
    assert!(matches!(event, ActorToTuiPacket::SessionResumed(Ok(_))));
}

fn summary(reply: oneshot::Sender<anyhow::Result<Events>>) {
    let mut events = response(vec![text(
        "Investigated src/lib.rs. The bug fix and final validation remain pending.",
    )]);
    if let Some(StreamEvent::MessageDelta { usage, .. }) = events.last_mut() {
        usage.input_tokens = 500;
        usage.output_tokens = 30;
    }
    answer(reply, events);
}

#[tokio::test]
async fn fitting_context_reports_tokens_and_continues_without_repeated_compaction() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let history = std::iter::once(llm::Message::new("workspace".into()))
        .chain((0..5).flat_map(|index| {
            [
                llm::Message::new(format!("Inspect file {index}")),
                llm::Message::new_assistant("inspected source ".repeat(300)),
            ]
        }))
        .collect();
    let session = store
        .create(llm::SessionProvider::Injected, None, history)
        .unwrap();
    let id = session.id.clone();
    drop(session);
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    for _ in 0..3 {
        h.start("Continue the implementation");
        let (request, reply) = h.request().await;
        assert!(
            request
                .system
                .as_deref()
                .unwrap()
                .starts_with("Follow the fixture")
        );
        let bytes = serde_json::to_vec(&request.messages).unwrap().len();
        assert!(bytes > configured_limits().trigger());
        let event = h
            .event(|packet| matches!(packet, ActorToTuiPacket::ContextUpdated(_)))
            .await;
        let ActorToTuiPacket::ContextUpdated(context) = event else {
            panic!("request context expected")
        };
        assert_eq!(
            context.estimated_tokens,
            estimated_tokens(&request).unwrap()
        );
        assert!(context.estimated_tokens < bytes / 2);
        assert_eq!(context.ceiling, configured_limits().ceiling());
        assert_eq!(context.response_reserve, configured_limits().response());
        answer(reply, response(vec![text("Implementation in progress")]));
        h.terminal(Lifecycle::Completed).await;
    }
    let snapshot = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(snapshot.context.generation, 0);
    h.stop().await;
}

#[tokio::test]
async fn automatic_compaction_survives_restart_and_forks() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let original = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap()
        .history;
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    h.start("Continue the implementation");
    let (request, reply) = h.request().await;
    assert!(request.system.unwrap().starts_with("Summarize only"));
    summary(reply);
    let (request, reply) = h.request().await;
    assert!(estimated_tokens(&request).unwrap() <= configured_limits().trigger());
    let sent = serde_json::to_string(&request.messages).unwrap();
    assert!(sent.contains("Keep public APIs unchanged"));
    assert!(sent.contains("final validation remain pending"));
    assert!(sent.contains("Investigation 4"));
    assert!(!sent.contains("Investigation 0"));
    assert!(sent.contains("workspace revision 1"));
    assert!(!sent.contains("obsolete workspace context"));
    answer(reply, response(vec![text("implementation in progress")]));
    h.terminal(Lifecycle::Completed).await;
    let saved = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(saved.context.generation, 1);
    assert_eq!(saved.usage.input_tokens, 500);
    assert_eq!(saved.history.len(), original.len() + 2);
    h.stop().await;
    drop(store);
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    h.actor
        .send_message(Message::Command(Command::Fork))
        .unwrap();
    let forked = h
        .event(|packet| matches!(packet, ActorToTuiPacket::CommandResult(Command::Fork, _)))
        .await;
    assert!(
        matches!(forked, ActorToTuiPacket::CommandResult(_, message) if message.contains("filesystem changes are shared"))
    );
    let fork = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.forked_from.as_deref() == Some(&id))
        .unwrap();
    assert_eq!(fork.context.generation, 1);
    h.start("Work on the fork");
    let (request, reply) = h.request().await;
    assert!(request.system.unwrap().starts_with("Follow the fixture"));
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains("Keep public APIs unchanged"))
    );
    answer(reply, response(vec![text("fork response")]));
    h.terminal(Lifecycle::Completed).await;
    assert!(
        !store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap()
            .history
            .iter()
            .any(|message| message.text() == "fork response")
    );
    h.stop().await;
}

#[tokio::test]
async fn manual_compaction_preserves_the_transcript_and_queued_followups() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    let before = serde_json::to_value(h.history().await).unwrap();
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (_, reply) = h.request().await;
    h.start("Keep the corrected requirement too");
    h.event(|packet| matches!(packet, ActorToTuiPacket::Queued { .. }))
        .await;
    summary(reply);
    h.terminal(Lifecycle::Completed).await;
    let (request, reply) = h.request().await;
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text() == "Keep the corrected requirement too")
    );
    let history = h.history().await;
    assert_eq!(
        serde_json::to_value(&history[..history.len() - 1]).unwrap(),
        before
    );
    answer(reply, response(vec![text("queued work continued")]));
    h.terminal(Lifecycle::Completed).await;
    assert_eq!(
        store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap()
            .context
            .generation,
        1
    );
    h.stop().await;
}

#[tokio::test]
async fn cancelled_compaction_cannot_commit() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (_, reply) = h.request().await;
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(reply.is_closed());
    assert_eq!(
        store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
            .unwrap()
            .context
            .generation,
        0
    );
    h.stop().await;
}

#[tokio::test]
async fn failed_or_malformed_summaries_leave_history_intact_and_can_be_retried() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::with_runtime(vec![write], runtime).await;
    resume(&h, &id).await;
    let before = serde_json::to_value(h.history().await).unwrap();
    let invalid_stops = [llm::StopReason::Refusal, llm::StopReason::MaxTokens]
        .into_iter()
        .map(|reason| {
            let mut events = response(vec![text("incomplete summary")]);
            if let Some(StreamEvent::MessageDelta { delta, .. }) = events.last_mut() {
                delta.stop_reason = Some(reason);
            }
            events
        });
    let mut late_content = response(vec![text("initial summary")]);
    late_content.push(StreamEvent::ContentBlockComplete {
        index: 1,
        content: text("content after completion"),
    });
    for events in [
        response(vec![call("write", "forbidden")]),
        vec![StreamEvent::MessageStart {
            message: llm::StreamMessage {
                id: "truncated".into(),
                model: "fixture".into(),
                role: llm::Role::Assistant,
                usage: Default::default(),
            },
        }],
        response(vec![text(&"oversized".repeat(1000))]),
        response(vec![text(" ")]),
        late_content,
    ]
    .into_iter()
    .chain(invalid_stops)
    {
        h.actor
            .send_message(Message::Command(Command::Compact))
            .unwrap();
        let (request, reply) = h.request().await;
        assert!(request.tools.is_empty());
        answer(reply, events);
        h.terminal(Lifecycle::Failed).await;
        assert!(h.requests.is_empty());
        assert!(entered.is_empty());
        assert_eq!(serde_json::to_value(h.history().await).unwrap(), before);
        assert_eq!(
            store
                .list()
                .unwrap()
                .into_iter()
                .find(|snapshot| snapshot.id == id)
                .unwrap()
                .context
                .generation,
            0
        );
    }
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    summary(h.request().await.1);
    h.terminal(Lifecycle::Completed).await;
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn compaction_worker_reports_partial_usage_and_is_drained_on_shutdown() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    let session_count = store.list().unwrap().len();
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (request, reply) = h.request().await;
    assert!(request.tools.is_empty());
    assert!(!request.thinking);
    assert_eq!(store.list().unwrap().len(), session_count);
    assert!(
        h.runtime
            .scope
            .resources()
            .iter()
            .any(|resource| { resource.kind == utils::execution::ResourceKind::Worker })
    );
    let (events, receive) = flume::unbounded();
    let stream = futures::stream::unfold(receive, |receive| async move {
        receive
            .recv_async()
            .await
            .ok()
            .map(|event| (event, receive))
    })
    .boxed();
    assert!(reply.send(Ok(stream)).is_ok());
    events
        .send(Ok(StreamEvent::MessageStart {
            message: llm::StreamMessage {
                id: "summary-worker".into(),
                model: "fixture".into(),
                role: llm::Role::Assistant,
                usage: llm::StreamUsage {
                    input_tokens: 321,
                    ..Default::default()
                },
            },
        }))
        .unwrap();
    events
        .send(Ok(StreamEvent::ContentBlockComplete {
            index: 0,
            content: text("unfinished internal summary"),
        }))
        .unwrap();
    events
        .send(Ok(StreamEvent::MessageDelta {
            delta: llm::MessageDeltaContent { stop_reason: None },
            usage: llm::UsageDelta {
                input_tokens: 0,
                output_tokens: 12,
            },
        }))
        .unwrap();
    h.event(|packet| {
        matches!(packet, ActorToTuiPacket::TokensUpdated(usage)
        if usage.input_tokens == 321 && usage.output_tokens == 12)
    })
    .await;
    assert!(h.requests.is_empty());
    assert!(
        !h.history()
            .await
            .iter()
            .any(|message| message.text().contains("unfinished internal summary"))
    );
    h.stop().await;
    assert!(events.is_disconnected());
    let snapshot = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(snapshot.context.generation, 0);
    assert_eq!(snapshot.usage.input_tokens, 321);
    assert_eq!(snapshot.usage.output_tokens, 12);
}

#[tokio::test]
async fn giant_validation_output_is_bounded_and_retrievable_in_simple_and_worker_modes() {
    for delegated in [false, true] {
        let workspace = crate::session::tests::Workspace::new();
        let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
        let store = runtime.sessions.clone().unwrap();
        let (mut validation, entered) = gate("cargo_test", ToolEffect::Validate);
        Arc::get_mut(&mut validation).unwrap().outcome = GateOutcome::LargeValidation;
        let (delegate, child_requests) = delegate(vec![validation.clone()], false);
        let h = Harness::with_runtime(
            match delegated {
                true => vec![delegate],
                false => vec![validation],
            },
            runtime,
        )
        .await;
        h.start("Run the regression tests");
        let requests = match delegated {
            true => {
                answer(
                    h.request().await.1,
                    response(vec![call("delegate", "child")]),
                );
                &child_requests
            }
            false => &h.requests,
        };
        answer(
            within(requests.recv_async()).await.unwrap().1,
            response(vec![call("cargo_test", "large-test")]),
        );
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
        let (request, reply) = within(requests.recv_async()).await.unwrap();
        let output = request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .find_map(|content| match content {
                ContentBlock::ToolResult {
                    content,
                    is_error: Some(true),
                    ..
                } => Some(content),
                _ => None,
            })
            .unwrap();
        assert!(output.len() < 9000);
        assert!(output.contains("FAILED: regression in src/lib.rs"));
        let artifact = store
            .list()
            .unwrap()
            .into_iter()
            .flat_map(|snapshot| snapshot.artifacts)
            .next()
            .unwrap();
        assert!(artifact.bytes > 1024 * 1024);
        assert!(output.contains(&artifact.id));
        let mut read = call("read_artifact", "retrieve");
        if let ContentBlock::ToolBlock { input, .. } = &mut read {
            *input = json!({"id": artifact.id, "offset": 0, "bytes": 4096})
                .as_object()
                .unwrap()
                .clone();
        }
        answer(reply, response(vec![read.clone()]));
        let (request, reply) = within(requests.recv_async()).await.unwrap();
        assert!(
            serde_json::to_string(&request.messages)
                .unwrap()
                .contains("next_offset: Some(")
        );
        answer(
            reply,
            response(vec![text(&format!(
                "Test failed; inspect artifact {}",
                artifact.id
            ))]),
        );
        if delegated {
            answer(h.request().await.1, response(vec![read]));
            let (request, reply) = h.request().await;
            assert!(
                serde_json::to_string(&request.messages)
                    .unwrap()
                    .contains("next_offset: Some(")
            );
            answer(reply, response(vec![text("Reported worker test failure")]));
        }
        h.terminal(Lifecycle::Completed).await;
        h.stop().await;
    }
}

#[tokio::test]
async fn native_compaction_commits_and_replays_all_opaque_items_after_restart() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let (normal, requests) = flume::unbounded();
    let (compactions, native_requests) = flume::unbounded();
    let client = llm::LLmClient::Injected(Arc::new(NativeProvider {
        normal: Provider(normal),
        compactions,
    }));
    let h = Harness::with_client(vec![], runtime, client, requests).await;
    resume(&h, &id).await;
    h.start("Continue after native compaction");
    let compact = within(native_requests.recv_async()).await.unwrap();
    assert!(
        compact
            .request
            .messages
            .iter()
            .any(|message| message.text().contains("Investigation 0"))
    );
    assert!(
        !compact
            .request
            .messages
            .iter()
            .any(|message| message.text().contains("Investigation 4"))
    );
    let output = json!([
        {"type": "message", "role": "user", "content": "retained input", "future": "preserve"},
        {"type": "compaction", "encrypted_content": "opaque payload", "future": [1, {"x": true}]}
    ]);
    assert!(
        compact
            .reply
            .send(Ok(serde_json::from_value(
                json!({"output": output, "usage": {"input_tokens": 1200, "output_tokens": 200}})
            )
            .unwrap()))
            .is_ok()
    );
    let (request, reply) = h.request().await;
    let window = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|block| match block {
            ContentBlock::OpenAICompaction(window) => Some(window),
            _ => None,
        })
        .unwrap();
    assert_eq!(serde_json::to_value(window).unwrap(), output);
    answer(reply, response(vec![text("native continuation completed")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
    drop(store);
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    h.start("Resume the task");
    let (request, reply) = h.request().await;
    let window = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|block| match block {
            ContentBlock::OpenAICompaction(window) => Some(window),
            _ => None,
        })
        .unwrap();
    assert_eq!(serde_json::to_value(window).unwrap(), output);
    let snapshot = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(snapshot.usage.input_tokens, 1200);
    assert_eq!(snapshot.usage.output_tokens, 200);
    answer(reply, response(vec![text("resumed")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn compaction_storage_failure_stops_before_the_next_provider_request() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let id = saved_history(&store);
    let h = Harness::with_runtime(vec![], runtime).await;
    resume(&h, &id).await;
    let before = serde_json::to_value(h.history().await).unwrap();
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (_, reply) = h.request().await;
    crate::session::tests::invalidate(&store, &id);
    summary(reply);
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    assert_eq!(serde_json::to_value(h.history().await).unwrap(), before);
    h.stop().await;
}

#[tokio::test]
async fn oversized_mandatory_context_fails_without_calling_the_provider() {
    let h = Harness::with_runtime(
        vec![],
        Runtime {
            context_budget: ContextBudget::new(Some(4096), 1024).unwrap(),
            ..Runtime::default()
        },
    )
    .await;
    h.start(&"Keep this requirement exactly. ".repeat(1000));
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    assert!(h.history().await.last().unwrap().text().len() > 4096);
    h.stop().await;
}

#[tokio::test]
async fn delegated_workers_compact_between_complete_tool_exchanges() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = configured_runtime(&workspace);
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolEffect::Read);
    let (delegate, child_requests) = delegate(vec![read], false);
    let h = Harness::with_runtime(vec![delegate], runtime).await;
    h.start("Investigate the bug");
    answer(
        h.request().await.1,
        response(vec![call("delegate", "child")]),
    );
    let mut compactions = 0;
    for index in 0..8 {
        let (mut request, mut reply) = within(child_requests.recv_async()).await.unwrap();
        if request
            .system
            .as_deref()
            .unwrap()
            .starts_with("Summarize only")
        {
            compactions += 1;
            summary(reply);
            (request, reply) = within(child_requests.recv_async()).await.unwrap();
        }
        assert!(estimated_tokens(&request).unwrap() <= configured_limits().trigger());
        answer(
            reply,
            response(vec![
                text(&"Investigation details ".repeat(1000)),
                call("read", &format!("read-{index}")),
            ]),
        );
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
    }
    let (request, reply) = within(child_requests.recv_async()).await.unwrap();
    match request
        .system
        .as_deref()
        .unwrap()
        .starts_with("Summarize only")
    {
        true => {
            compactions += 1;
            summary(reply);
            answer(
                within(child_requests.recv_async()).await.unwrap().1,
                response(vec![text("Investigation finished")]),
            );
        }
        false => answer(reply, response(vec![text("Investigation finished")])),
    }
    answer(h.request().await.1, response(vec![text("Root completed")]));
    h.terminal(Lifecycle::Completed).await;
    assert!(compactions > 0);
    assert!(
        store
            .list()
            .unwrap()
            .iter()
            .any(|snapshot| snapshot.parent.is_some() && snapshot.context.generation > 0)
    );
    assert!(entered.is_empty());
    h.stop().await;
}
