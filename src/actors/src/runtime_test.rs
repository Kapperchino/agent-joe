use crate::{
    actor::{ActorContext, Dependency, Message},
    actor_state::ActorState,
    provider_task::ProviderEvent,
    runtime::Runtime,
    scheduler::ToolEvent,
    stream_replay_test::TestContext,
    turn::Tag,
    worker::{Worker, WorkerAdapter},
};
use async_trait::async_trait;
use clients::{
    failure::{Failure, FailureKind},
    llm::{self, ContentBlock, StreamEvent, StreamProvider},
};
use common_models::tui_models::{ActorToTui, ActorToTuiPacket, Lifecycle};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use ractor::{Actor, ActorProcessingErr, ActorRef};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::oneshot;
use tools::tool_defs::{ErasedToolRef, ErasedToolTrait, ToolDefinition, ToolEffect, ToolId};
use utils::utils::FnvHashMap;

type Events = BoxStream<'static, anyhow::Result<StreamEvent>>;
type Request = (llm::ClientRequest, oneshot::Sender<anyhow::Result<Events>>);
struct Provider(flume::Sender<Request>);
impl StreamProvider for Provider {
    fn chat_stream(
        &self,
        request: llm::ClientRequest,
    ) -> BoxFuture<'static, anyhow::Result<Events>> {
        let tx = self.0.clone();
        Box::pin(async move {
            let (reply, receive) = oneshot::channel();
            tx.send_async((request, reply)).await.unwrap();
            receive
                .await
                .map_err(anyhow::Error::from)
                .and_then(std::convert::identity)
        })
    }
}
struct FixtureWorker;
#[async_trait]
impl Worker for FixtureWorker {
    type C = TestContext;
    fn init_prompt(_: Option<&str>) -> String {
        "Fixture instructions".into()
    }
    fn tools() -> Vec<ErasedToolRef<TestContext, ActorContext<TestContext>>> {
        vec![]
    }
    async fn startup_hook(
        &self,
        actor: ActorRef<Message>,
        dependency: Dependency<TestContext>,
    ) -> Result<ActorState<TestContext>, ActorProcessingErr> {
        ActorState::new(dependency, actor, None)
            .await
            .map_err(|err| err.to_string().into())
    }
}

struct Harness {
    actor: ActorRef<Message>,
    handle: Option<tokio::task::JoinHandle<()>>,
    requests: flume::Receiver<Request>,
    events: flume::Receiver<ActorToTui>,
    runtime: Runtime,
}
impl Harness {
    async fn new(
        tools: Vec<ErasedToolRef<TestContext, ActorContext<TestContext>>>,
        timeout: Duration,
    ) -> Self {
        let (tx, requests) = flume::unbounded();
        let (tui_tx, events) = flume::unbounded();
        let runtime = Runtime {
            tool_timeout: timeout,
            ..Runtime::default()
        };
        let (actor, handle) = Actor::spawn(
            None,
            WorkerAdapter::new(FixtureWorker),
            Dependency {
                client: llm::LLmClient::Injected(Arc::new(Provider(tx))),
                tools,
                tui_tx,
                debug_mode: false,
                context: TestContext {
                    task: None,
                    revision: 1,
                },
                runtime: runtime.clone(),
            },
        )
        .await
        .unwrap();
        Self {
            actor,
            handle: Some(handle),
            requests,
            events,
            runtime,
        }
    }
    fn start(&self, prompt: &str) {
        self.actor
            .send_message(Message::StartWork(Some(prompt.into())))
            .unwrap();
    }
    async fn request(&self) -> Request {
        within(self.requests.recv_async()).await.unwrap()
    }
    async fn event(&self, predicate: impl Fn(&ActorToTuiPacket) -> bool) -> ActorToTuiPacket {
        within(async {
            loop {
                let event = self.events.recv_async().await.unwrap().packet;
                if predicate(&event) {
                    break event;
                }
            }
        })
        .await
    }
    async fn terminal(&self, expected: Lifecycle) {
        let event = self.event(|packet| matches!(packet, ActorToTuiPacket::TurnChanged { state, .. } if state.terminal())).await;
        assert!(
            matches!(event, ActorToTuiPacket::TurnChanged { state, .. } if state == expected),
            "{event:?}"
        );
    }
    async fn history(&self) -> Vec<llm::Message> {
        let (tx, rx) = oneshot::channel();
        self.actor
            .send_message(Message::Inspect(tx.into()))
            .unwrap();
        within(rx).await.unwrap()
    }
    async fn stop(mut self) {
        self.actor.stop(None);
        within(self.handle.take().unwrap()).await.unwrap();
        assert_eq!(self.runtime.scope.tasks.len(), 0);
        assert!(self.runtime.scope.resources().is_empty());
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.actor.stop(None);
    }
}
async fn within<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("fixture timed out")
}
fn answer(reply: oneshot::Sender<anyhow::Result<Events>>, events: Vec<StreamEvent>) {
    assert!(
        reply
            .send(Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed()))
            .is_ok()
    );
}
fn response(blocks: Vec<ContentBlock>) -> Vec<StreamEvent> {
    let tools = blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolBlock { .. }));
    let mut events = vec![StreamEvent::MessageStart {
        message: llm::StreamMessage {
            id: "fixture".into(),
            model: "fixture".into(),
            role: llm::Role::Assistant,
            usage: Default::default(),
        },
    }];
    events.extend(
        blocks
            .into_iter()
            .enumerate()
            .map(|(index, content)| StreamEvent::ContentBlockComplete { index, content }),
    );
    events.push(StreamEvent::MessageDelta {
        delta: llm::MessageDeltaContent {
            stop_reason: Some(if tools {
                llm::StopReason::ToolUse
            } else {
                llm::StopReason::EndTurn
            }),
        },
        usage: llm::UsageDelta {
            input_tokens: 0,
            output_tokens: 0,
        },
    });
    events
}
fn text(message: &str) -> ContentBlock {
    ContentBlock::MessageBlock {
        text: message.into(),
        phase: None,
    }
}
fn call(name: &str, id: &str) -> ContentBlock {
    ContentBlock::ToolBlock {
        tool_id: ToolId {
            id: id.to_owned().try_into().unwrap(),
            call_id: None,
        },
        name: name.to_owned().try_into().unwrap(),
        input: json!({"id":id}).as_object().unwrap().clone(),
    }
}

struct GateTool {
    name: &'static str,
    effect: ToolEffect,
    entered: flume::Sender<(String, oneshot::Sender<()>)>,
    active: Arc<AtomicUsize>,
    outcome: GateOutcome,
}
#[derive(Clone, Copy)]
enum GateOutcome {
    Success,
    Failure,
    PreparePanic,
    RunPanic,
    RenderPanic,
    ContextFailure,
    ContextPanic,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
#[async_trait]
impl ErasedToolTrait<TestContext, ActorContext<TestContext>> for GateTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Client {
            name: self.name.into(),
            description: "fixture".into(),
            properties: Default::default(),
            required: vec![],
        }
    }
    fn effect(&self) -> ToolEffect {
        self.effect
    }
    fn display_erased(&self, _: &Value) -> anyhow::Result<String> {
        match self.outcome {
            GateOutcome::PreparePanic => panic!("fixture preparation panic"),
            _ => Ok(self.name.into()),
        }
    }
    fn input_req_erased(&self, _: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    async fn run_erased(
        &self,
        input: Value,
        _: ToolId,
        _: &TestContext,
        _: &ActorContext<TestContext>,
    ) -> anyhow::Result<Value> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(self.active.clone());
        let (tx, rx) = oneshot::channel();
        self.entered
            .send((input["id"].as_str().unwrap().into(), tx))
            .unwrap();
        let _ = rx.await;
        match self.outcome {
            GateOutcome::Failure => Err(anyhow::anyhow!("fixture failure")),
            GateOutcome::RunPanic => panic!("fixture execution panic"),
            _ => Ok(input),
        }
    }
    fn output_to_content_erased(&self, _: &Value, output: &Value) -> anyhow::Result<String> {
        match self.outcome {
            GateOutcome::RenderPanic => panic!("fixture output panic"),
            _ => Ok(output.to_string()),
        }
    }
    fn add_context(&self, _: &Value, _: &mut TestContext, _: &str) -> anyhow::Result<()> {
        match self.outcome {
            GateOutcome::ContextFailure => Err(anyhow::anyhow!("fixture context failure")),
            GateOutcome::ContextPanic => panic!("fixture context panic"),
            _ => Ok(()),
        }
    }
}
fn gate(
    name: &'static str,
    effect: ToolEffect,
) -> (
    Arc<GateTool>,
    flume::Receiver<(String, oneshot::Sender<()>)>,
) {
    let (entered, rx) = flume::unbounded();
    (
        Arc::new(GateTool {
            name,
            effect,
            entered,
            active: Default::default(),
            outcome: GateOutcome::Success,
        }),
        rx,
    )
}

#[tokio::test]
async fn cancel_during_request_and_ignore_obsolete_events() {
    let h = Harness::new(vec![], Duration::from_secs(10)).await;
    h.start("first");
    let (_, reply) = h.request().await;
    let packet = h
        .event(|event| {
            matches!(
                event,
                ActorToTuiPacket::OperationChanged {
                    state: Lifecycle::Running,
                    ..
                }
            )
        })
        .await;
    let tag = match packet {
        ActorToTuiPacket::OperationChanged {
            turn_id,
            operation_id,
            ..
        } => Tag {
            turn: turn_id,
            operation: operation_id,
        },
        _ => unreachable!(),
    };
    h.actor.send_message(Message::Interrupt).unwrap();
    h.event(|event| {
        matches!(event, ActorToTuiPacket::OperationChanged {
        operation_id, state: Lifecycle::Cancelled, ..
    } if *operation_id == tag.operation)
    })
    .await;
    h.terminal(Lifecycle::Cancelled).await;
    assert!(reply.is_closed());
    h.start("second");
    let (_, reply) = h.request().await;
    for event in response(vec![text("obsolete")]) {
        h.actor
            .send_message(Message::Provider {
                tag,
                event: ProviderEvent::Item(event),
            })
            .unwrap();
    }
    h.actor
        .send_message(Message::Provider {
            tag,
            event: ProviderEvent::Finished(Ok(())),
        })
        .unwrap();
    answer(reply, response(vec![text("current")]));
    h.terminal(Lifecycle::Completed).await;
    let history = h.history().await;
    assert!(
        history
            .iter()
            .all(|message| !message.text().contains("obsolete"))
    );
    assert_eq!(history.last().unwrap().text(), "current");
    h.stop().await;
}

#[tokio::test]
async fn followups_queue_without_overlapping_streams_or_duplicate_tools() {
    let (tool, entered) = gate("read", ToolEffect::Read);
    let h = Harness::new(vec![tool], Duration::from_secs(10)).await;
    h.start("first");
    let (_, reply) = h.request().await;
    h.start("follow-up");
    h.event(|event| matches!(event, ActorToTuiPacket::Queued { position: 1, .. }))
        .await;
    assert!(h.requests.is_empty());
    answer(reply, response(vec![call("read", "once")]));
    within(entered.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let (request, reply) = h.request().await;
    assert!(
        request
            .messages
            .iter()
            .all(|message| message.text() != "follow-up")
    );
    answer(reply, response(vec![text("first done")]));
    h.terminal(Lifecycle::Completed).await;
    let (request, reply) = h.request().await;
    assert_eq!(request.messages.last().unwrap().text(), "follow-up");
    answer(reply, response(vec![text("second done")]));
    h.terminal(Lifecycle::Completed).await;
    assert!(entered.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn reads_overlap_writes_are_ordered_and_validation_uses_latest_revision() {
    let (read, reads) = gate("read", ToolEffect::Read);
    let (write, writes) = gate("write", ToolEffect::Write);
    let (validate, validations) = gate("validate", ToolEffect::Validate);
    let h = Harness::new(vec![read.clone(), write, validate], Duration::from_secs(10)).await;
    h.start("work");
    answer(
        h.request().await.1,
        response(vec![
            call("read", "r1"),
            call("read", "r2"),
            call("write", "w1"),
            call("write", "w2"),
            call("validate", "v"),
        ]),
    );
    let r1 = within(reads.recv_async()).await.unwrap();
    let r2 = within(reads.recv_async()).await.unwrap();
    assert_eq!(read.active.load(Ordering::SeqCst), 2);
    assert!(writes.is_empty());
    r1.1.send(()).unwrap();
    r2.1.send(()).unwrap();
    let w1 = within(writes.recv_async()).await.unwrap();
    assert_eq!(w1.0, "w1");
    assert!(writes.is_empty());
    assert!(validations.is_empty());
    w1.1.send(()).unwrap();
    let w2 = within(writes.recv_async()).await.unwrap();
    assert_eq!(w2.0, "w2");
    assert_eq!(h.runtime.workspace.revision().to_string(), "1");
    w2.1.send(()).unwrap();
    let validation = within(validations.recv_async()).await.unwrap();
    assert_eq!(h.runtime.workspace.revision().to_string(), "2");
    validation.1.send(()).unwrap();
    answer(h.request().await.1, response(vec![text("done")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn cancel_tools_retains_success_and_marks_unexecuted_calls() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![write.clone()], Duration::from_secs(10)).await;
    h.start("work");
    answer(
        h.request().await.1,
        response(vec![
            call("write", "done"),
            call("write", "interrupted"),
            call("write", "never"),
        ]),
    );
    within(entered.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(pending.is_closed());
    assert!(entered.is_empty());
    assert_eq!(write.active.load(Ordering::SeqCst), 0);
    let history = h.history().await;
    let outputs = &history.last().unwrap().content;
    assert!(matches!(
        &outputs[0],
        ContentBlock::ToolResult { is_error: None, .. }
    ));
    assert!(
        matches!(&outputs[1], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("partial"))
    );
    assert!(
        matches!(&outputs[2], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("Not executed"))
    );
    h.stop().await;
}

#[tokio::test]
async fn timeout_stops_uncertain_write_and_next_write_is_never_run() {
    let (tool, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![tool.clone()], Duration::from_millis(30)).await;
    h.start("work");
    answer(
        h.request().await.1,
        response(vec![call("write", "timeout"), call("write", "never")]),
    );
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    h.terminal(Lifecycle::Failed).await;
    assert!(pending.is_closed());
    assert!(entered.is_empty());
    assert!(h.requests.is_empty());
    assert_eq!(tool.active.load(Ordering::SeqCst), 0);
    h.stop().await;
}

#[tokio::test]
async fn read_timeout_stops_the_batch_before_a_write_starts() {
    let (read, reads) = gate("read", ToolEffect::Read);
    let (write, writes) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![read.clone(), write], Duration::from_millis(30)).await;
    h.start("work");
    answer(
        h.request().await.1,
        response(vec![call("read", "timeout"), call("write", "never")]),
    );
    let (_, pending) = within(reads.recv_async()).await.unwrap();
    h.terminal(Lifecycle::Failed).await;
    assert!(pending.is_closed());
    assert!(writes.is_empty());
    assert!(h.requests.is_empty());
    assert_eq!(read.active.load(Ordering::SeqCst), 0);
    let history = h.history().await;
    let outputs = &history.last().unwrap().content;
    assert!(
        matches!(&outputs[0], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.starts_with("Timeout:"))
    );
    assert!(
        matches!(&outputs[1], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.starts_with("Not executed:"))
    );
    h.stop().await;
}

#[tokio::test]
async fn tool_panics_preserve_other_reads_and_stop_subsequent_writes() {
    for outcome in [
        GateOutcome::PreparePanic,
        GateOutcome::RunPanic,
        GateOutcome::RenderPanic,
    ] {
        let (mut panicking, entered) = gate("panicking", ToolEffect::Read);
        Arc::get_mut(&mut panicking).unwrap().outcome = outcome;
        let (read, reads) = gate("read", ToolEffect::Read);
        let (write, writes) = gate("write", ToolEffect::Write);
        let h = Harness::new(
            vec![panicking, read.clone(), write],
            Duration::from_secs(10),
        )
        .await;
        h.start("work");
        answer(
            h.request().await.1,
            response(vec![
                call("panicking", "panic"),
                call("read", "done"),
                call("write", "never"),
            ]),
        );
        let (_, pending_read) = within(reads.recv_async()).await.unwrap();
        if !matches!(outcome, GateOutcome::PreparePanic) {
            within(entered.recv_async())
                .await
                .unwrap()
                .1
                .send(())
                .unwrap();
        }
        h.event(|event| {
            matches!(event, ActorToTuiPacket::OperationChanged {
            state: Lifecycle::Failed, detail, ..
        } if detail.starts_with("Panicked:"))
        })
        .await;
        assert_eq!(read.active.load(Ordering::SeqCst), 1);
        pending_read.send(()).unwrap();
        h.terminal(Lifecycle::Failed).await;
        let history = h.history().await;
        let outputs = &history.last().unwrap().content;
        assert!(
            matches!(&outputs[0], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.starts_with("Panicked:"))
        );
        assert!(matches!(
            &outputs[1],
            ContentBlock::ToolResult { is_error: None, .. }
        ));
        assert!(
            matches!(&outputs[2], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.starts_with("Not executed:"))
        );
        assert!(writes.is_empty());
        assert!(h.requests.is_empty());
        assert!(h.runtime.workspace.is_idle());
        h.stop().await;
    }
}

#[tokio::test]
async fn context_update_feedback_stops_before_the_next_provider_request() {
    for outcome in [GateOutcome::ContextFailure, GateOutcome::ContextPanic] {
        let (mut read, entered) = gate("read", ToolEffect::Read);
        Arc::get_mut(&mut read).unwrap().outcome = outcome;
        let h = Harness::new(vec![read], Duration::from_secs(10)).await;
        h.start("work");
        answer(h.request().await.1, response(vec![call("read", "done")]));
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
        h.terminal(Lifecycle::Failed).await;
        assert!(h.requests.is_empty());
        let history = h.history().await;
        assert!(matches!(&history.last().unwrap().content[0],
            ContentBlock::ToolResult { content, is_error: None, .. } if content.contains("done")
        ));
        assert!(h.runtime.workspace.is_idle());
        h.stop().await;
    }
}

#[tokio::test]
async fn interrupted_provider_retries_only_unaccepted_response_and_bounds_failures() {
    let h = Harness::new(vec![], Duration::from_secs(10)).await;
    h.start("work");
    for _ in 0..3 {
        let (request, reply) = h.request().await;
        assert_eq!(request.messages.len(), 2);
        let mut events = response(vec![text("partial")]);
        events.pop();
        answer(reply, events);
    }
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    assert_eq!(h.history().await.last().unwrap().text(), "partial");
    h.stop().await;
}

#[tokio::test]
async fn authentication_error_is_visible_and_does_not_retry() {
    let h = Harness::new(vec![], Duration::from_secs(10)).await;
    h.start("work");
    assert!(
        h.request()
            .await
            .1
            .send(Err(Failure::new(
                FailureKind::Authentication,
                "fixture credentials rejected"
            )
            .into()))
            .is_ok()
    );
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    h.stop().await;
}

struct DelegateTool {
    client: llm::LLmClient,
    tools: Vec<ErasedToolRef<TestContext, ActorContext<TestContext>>>,
    panic_start: bool,
}
#[async_trait]
impl ErasedToolTrait<TestContext, ActorContext<TestContext>> for DelegateTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Client {
            name: "delegate".into(),
            description: "fixture worker".into(),
            properties: Default::default(),
            required: vec![],
        }
    }
    fn effect(&self) -> ToolEffect {
        ToolEffect::DelegateWrite
    }
    fn display_erased(&self, _: &Value) -> anyhow::Result<String> {
        Ok("delegate".into())
    }
    fn input_req_erased(&self, _: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    async fn run_erased(
        &self,
        _: Value,
        _: ToolId,
        context: &TestContext,
        actor: &ActorContext<TestContext>,
    ) -> anyhow::Result<Value> {
        let ActorContext::ActorInfo(info) = actor else {
            unreachable!()
        };
        let dependency = Dependency {
            client: self.client.clone(),
            tools: self.tools.clone(),
            context: context.clone(),
            tui_tx: info.dep.tui_tx.clone(),
            debug_mode: false,
            runtime: info.dep.runtime.child(info.dep.runtime.scope.child()),
        };
        let result = if self.panic_start {
            crate::worker::run_worker(FailingWorker, dependency, info.actor_ref.clone())
                .await
                .map_err(|error| error.into_tool_failure().into())
        } else {
            crate::worker::run_worker(FixtureWorker, dependency, info.actor_ref.clone())
                .await
                .map_err(|error| error.into_tool_failure().into())
        };
        result.map(|result| json!(result))
    }
    fn output_to_content_erased(&self, _: &Value, output: &Value) -> anyhow::Result<String> {
        Ok(output.to_string())
    }
    fn add_context(&self, _: &Value, _: &mut TestContext, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
struct FailingWorker;
#[async_trait]
impl Worker for FailingWorker {
    type C = TestContext;
    fn init_prompt(_: Option<&str>) -> String {
        String::new()
    }
    fn tools() -> Vec<ErasedToolRef<TestContext, ActorContext<TestContext>>> {
        vec![]
    }
    async fn startup_hook(
        &self,
        _: ActorRef<Message>,
        _: Dependency<TestContext>,
    ) -> Result<ActorState<TestContext>, ActorProcessingErr> {
        Err("fixture child startup failure".into())
    }
}
fn delegate(
    tools: Vec<ErasedToolRef<TestContext, ActorContext<TestContext>>>,
    panic_start: bool,
) -> (Arc<DelegateTool>, flume::Receiver<Request>) {
    let (tx, requests) = flume::unbounded();
    (
        Arc::new(DelegateTool {
            client: llm::LLmClient::Injected(Arc::new(Provider(tx))),
            tools,
            panic_start,
        }),
        requests,
    )
}

#[tokio::test]
async fn delegated_write_cancels_child_and_resolves_parent_after_cleanup() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let (delegate, child_requests) = delegate(vec![write.clone()], false);
    let h = Harness::new(vec![delegate], Duration::from_secs(10)).await;
    h.start("delegate work");
    answer(
        h.request().await.1,
        response(vec![call("delegate", "child")]),
    );
    let (_, reply) = within(child_requests.recv_async()).await.unwrap();
    answer(reply, response(vec![call("write", "interrupted")]));
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    h.terminal(Lifecycle::Cancelled).await;
    assert!(pending.is_closed());
    assert_eq!(write.active.load(Ordering::SeqCst), 0);
    assert!(child_requests.is_empty());
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn child_failure_resolves_parent_without_hanging() {
    let (delegate, _) = delegate(vec![], true);
    let h = Harness::new(vec![delegate], Duration::from_secs(10)).await;
    h.start("delegate work");
    answer(
        h.request().await.1,
        response(vec![call("delegate", "child")]),
    );
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    assert!(
        matches!(&h.history().await.last().unwrap().content[0], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("startup"))
    );
    h.stop().await;
}

#[tokio::test]
async fn immediately_completed_worker_registers_reply_before_starting() {
    let (delegate, child_requests) = delegate(vec![], false);
    let h = Harness::new(vec![delegate], Duration::from_secs(10)).await;
    h.start("delegate work");
    answer(
        h.request().await.1,
        response(vec![call("delegate", "child")]),
    );
    answer(
        within(child_requests.recv_async()).await.unwrap().1,
        response(vec![text("child result")]),
    );
    h.terminal(Lifecycle::Completed).await;
    let (request, reply) = h.request().await;
    assert!(
        matches!(&request.messages.last().unwrap().content[0], ContentBlock::ToolResult { content, is_error: None, .. } if content.contains("child result"))
    );
    answer(reply, response(vec![text("parent result")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn repeated_identical_read_failures_stop_after_three_attempts() {
    let (mut tool, entered) = gate("read", ToolEffect::Read);
    Arc::get_mut(&mut tool).unwrap().outcome = GateOutcome::Failure;
    let h = Harness::new(vec![tool], Duration::from_secs(10)).await;
    h.start("work");
    for _ in 0..3 {
        answer(h.request().await.1, response(vec![call("read", "same")]));
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
    }
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn clear_during_tools_cancels_before_rebuilding_context() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![write], Duration::from_secs(10)).await;
    h.start("old work");
    answer(h.request().await.1, response(vec![call("write", "active")]));
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    h.actor.send_message(Message::Clear).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(pending.is_closed());
    h.event(|event| {
        matches!(
            event,
            ActorToTuiPacket::CommandResult(commands::command::Command::Clear, _)
        )
    })
    .await;
    let history = h.history().await;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].text(), "workspace revision 1");
    h.stop().await;
}

#[tokio::test]
async fn late_tool_completion_after_cancellation_does_not_modify_a_new_turn() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![write], Duration::from_secs(10)).await;
    h.start("old work");
    answer(h.request().await.1, response(vec![call("write", "old")]));
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    let event = h
        .event(|event| {
            matches!(
                event,
                ActorToTuiPacket::OperationChanged {
                    state: Lifecycle::WaitingForTools,
                    ..
                }
            )
        })
        .await;
    let tag = match event {
        ActorToTuiPacket::OperationChanged {
            turn_id,
            operation_id,
            ..
        } => Tag {
            turn: turn_id,
            operation: operation_id,
        },
        _ => unreachable!(),
    };
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(pending.is_closed());
    h.start("new work");
    let (_, reply) = h.request().await;
    let ContentBlock::ToolBlock {
        tool_id,
        name,
        input,
    } = call("write", "old")
    else {
        unreachable!()
    };
    h.actor
        .send_message(Message::Tools {
            tag,
            event: ToolEvent::Completed {
                operation: common_models::runtime_ids::OperationId::new(),
                result: crate::tool_call::ToolCall {
                    id: tool_id,
                    name,
                    input,
                }
                .failed(tools::tool_error::ToolFailure::new(
                    tools::tool_error::ToolFailureKind::Execution,
                    tools::tool_error::ToolEffects::NoWorkspaceChange,
                    "obsolete result",
                )),
            },
        })
        .unwrap();
    h.actor
        .send_message(Message::Tools {
            tag,
            event: ToolEvent::Finished(Ok(())),
        })
        .unwrap();
    answer(reply, response(vec![text("new result")]));
    h.terminal(Lifecycle::Completed).await;
    assert!(
        h.history()
            .await
            .iter()
            .all(|message| !message.to_string().contains("obsolete result"))
    );
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn child_provider_failure_after_start_resolves_parent() {
    let (delegate, child_requests) = delegate(vec![], false);
    let h = Harness::new(vec![delegate], Duration::from_secs(10)).await;
    h.start("delegate");
    answer(
        h.request().await.1,
        response(vec![call("delegate", "child")]),
    );
    let (_, reply) = within(child_requests.recv_async()).await.unwrap();
    assert!(
        reply
            .send(Err(Failure::new(
                FailureKind::Authentication,
                "child auth failed"
            )
            .into()))
            .is_ok()
    );
    h.terminal(Lifecycle::Failed).await;
    h.terminal(Lifecycle::Failed).await;
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn read_concurrency_remains_bounded() {
    let (read, entered) = gate("read", ToolEffect::Read);
    let h = Harness::new(vec![read.clone()], Duration::from_secs(10)).await;
    h.start("read");
    answer(
        h.request().await.1,
        response((0..6).map(|id| call("read", &id.to_string())).collect()),
    );
    let mut pending = Vec::new();
    for _ in 0..4 {
        pending.push(within(entered.recv_async()).await.unwrap().1);
    }
    assert_eq!(read.active.load(Ordering::SeqCst), 4);
    assert!(entered.is_empty());
    for reply in pending {
        reply.send(()).unwrap();
    }
    for _ in 0..2 {
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
    }
    answer(h.request().await.1, response(vec![text("done")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[cfg(unix)]
#[test]
fn process_fixture() {
    if let Some(marker) = std::env::var_os("JOE_M2_ACTOR_PROCESS_MARKER") {
        std::fs::write(marker, std::process::id().to_string()).unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(unix)]
struct ProcessTool(std::path::PathBuf);
#[cfg(unix)]
#[async_trait]
impl ErasedToolTrait<TestContext, ActorContext<TestContext>> for ProcessTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Client {
            name: "test_process".into(),
            description: "managed test process".into(),
            properties: Default::default(),
            required: vec![],
        }
    }
    fn effect(&self) -> ToolEffect {
        ToolEffect::Validate
    }
    fn display_erased(&self, _: &Value) -> anyhow::Result<String> {
        Ok("test_process".into())
    }
    fn input_req_erased(&self, _: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(Default::default())
    }
    async fn run_erased(
        &self,
        _: Value,
        _: ToolId,
        _: &TestContext,
        _: &ActorContext<TestContext>,
    ) -> anyhow::Result<Value> {
        match std::env::current_exe() {
            Ok(executable) => {
                let mut command = tokio::process::Command::new(executable);
                command
                    .args(["--exact", "runtime_test::process_fixture", "--nocapture"])
                    .env("JOE_M2_ACTOR_PROCESS_MARKER", &self.0);
                utils::process::output(command)
                    .await
                    .map(|result| json!(result.status.success()))
            }
            Err(error) => Err(error.into()),
        }
    }
    fn output_to_content_erased(&self, _: &Value, output: &Value) -> anyhow::Result<String> {
        Ok(output.to_string())
    }
    fn add_context(&self, _: &Value, _: &mut TestContext, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
#[tokio::test]
async fn interrupt_reaps_a_running_test_process_before_publishing_cancelled() {
    let marker = std::env::temp_dir().join(format!(
        "joe-m2-actor-process-{}-{}",
        std::process::id(),
        common_models::runtime_ids::OperationId::new()
    ));
    let h = Harness::new(
        vec![Arc::new(ProcessTool(marker.clone()))],
        Duration::from_secs(10),
    )
    .await;
    h.start("run test");
    answer(
        h.request().await.1,
        response(vec![call("test_process", "test")]),
    );
    within(async {
        loop {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        h.runtime
            .scope
            .resources()
            .iter()
            .any(|resource| resource.kind == utils::execution::ResourceKind::Process)
    );
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(h.runtime.scope.resources().is_empty());
    assert!(h.runtime.workspace.is_idle());
    std::fs::remove_file(marker).unwrap();
    h.stop().await;
}

#[tokio::test]
async fn failed_validation_is_an_error_with_diagnostics_in_history() {
    struct Validation;
    #[async_trait]
    impl ErasedToolTrait<TestContext, ActorContext<TestContext>> for Validation {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::Client {
                name: "validate".into(),
                description: String::new(),
                properties: Default::default(),
                required: vec![],
            }
        }
        fn effect(&self) -> ToolEffect {
            ToolEffect::Validate
        }
        fn display_erased(&self, _: &Value) -> anyhow::Result<String> {
            Ok("validate".into())
        }
        fn input_req_erased(&self, _: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
            Ok(Default::default())
        }
        async fn run_erased(
            &self,
            _: Value,
            id: ToolId,
            _: &TestContext,
            _: &ActorContext<TestContext>,
        ) -> anyhow::Result<Value> {
            serde_json::to_value(tools::cargo_test::CargoTestToolResult {
                id,
                status: "failed".into(),
                result: tools::cargo_test::CargoTestResult::Failed {
                    output: "regression assertion failed".into(),
                },
            })
            .map_err(Into::into)
        }
        fn output_to_content_erased(
            &self,
            input: &Value,
            output: &Value,
        ) -> anyhow::Result<String> {
            tools::tool_defs::erased_tool::<
                tools::cargo_test::CargoTest,
                TestContext,
                ActorContext<TestContext>,
            >()
            .output_to_content_erased(input, output)
        }
        fn output_is_error_erased(&self, output: &Value) -> anyhow::Result<bool> {
            tools::tool_defs::erased_tool::<
                tools::cargo_test::CargoTest,
                TestContext,
                ActorContext<TestContext>,
            >()
            .output_is_error_erased(output)
        }
        fn add_context(&self, _: &Value, _: &mut TestContext, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let h = Harness::new(vec![Arc::new(Validation)], Duration::from_secs(10)).await;
    h.start("validate");
    answer(
        h.request().await.1,
        response(vec![call("validate", "failure")]),
    );
    let (request, reply) = h.request().await;
    assert!(
        matches!(&request.messages.last().unwrap().content[0], ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("regression assertion failed"))
    );
    answer(reply, response(vec![text("reported failure")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn shutdown_waits_for_active_tool_cleanup() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![write.clone()], Duration::from_secs(10)).await;
    h.start("work");
    answer(h.request().await.1, response(vec![call("write", "active")]));
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    let events = h.events.clone();
    h.stop().await;
    assert!(pending.is_closed());
    assert_eq!(write.active.load(Ordering::SeqCst), 0);
    assert!(events.drain().any(|event| matches!(
        event.packet,
        ActorToTuiPacket::TurnChanged {
            state: Lifecycle::Cancelled,
            ..
        }
    )));
}
