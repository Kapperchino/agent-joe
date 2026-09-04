use crate::actor::{ActorContext, Dependency, Message};
use crate::actor_state::ActorState;
use crate::stream_processor::StreamNextStep;
use analysis::contexts::context::Context;
use analysis::contexts::rust_context::RustContextLineIndexCreator;
use async_trait::async_trait;
use clients::config::{Config, ConfigContext};
use clients::llm::{self, LLmClient};
use clients::{LocalOpenAIConfig, OpenAIAuthConfig, OpenAIConfig, OpenAIEffort, claude, openai};
use ractor::{Actor, ActorProcessingErr, ActorRef};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tools::tool_defs::{ErasedToolTrait, ToolDefinition, ToolId};
use utils::utils::FnvHashMap;

#[derive(Clone)]
struct TestContext {
    task: Option<String>,
    revision: usize,
}

#[async_trait]
impl Context for TestContext {
    type LineIndexCreator = RustContextLineIndexCreator;
    async fn get_ctx(&self) -> String {
        format!("workspace revision {}", self.revision)
    }
    fn instructions(&self) -> &str {
        "Follow the fixture's operating instructions."
    }
    fn initial_task(&self) -> Option<&str> {
        self.task.as_deref()
    }
    fn clear_task_context(&mut self) {
        self.task = None;
    }
    fn get_root(&self) -> PathBuf {
        PathBuf::new()
    }
    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(vec![])
    }
    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        anyhow::bail!("The fixture does not use semantic indexing")
    }
    fn gen_id(&self) -> u64 {
        1
    }
    fn get_id(&self) -> u64 {
        1
    }
}

struct IdleActor;
impl Actor for IdleActor {
    type Msg = Message;
    type State = ();
    type Arguments = ();
    async fn pre_start(&self, _: ActorRef<Message>, _: ()) -> Result<(), ActorProcessingErr> {
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

struct EchoTool(Arc<AtomicUsize>);
#[async_trait]
impl ErasedToolTrait<TestContext, ActorContext<TestContext>> for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::Client {
            name: "echo".into(),
            description: "A deterministic fixture".into(),
            properties: Default::default(),
            required: vec![],
        }
    }
    fn display_erased(&self, _: &Value) -> anyhow::Result<String> {
        Ok("echo".into())
    }
    fn input_req_erased(&self, _: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
        anyhow::bail!("History must not rebuild input through the lossy string-map path")
    }
    async fn run_erased(
        &self,
        input: Value,
        _: ToolId,
        _: &TestContext,
        _: &ActorContext<TestContext>,
    ) -> anyhow::Result<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(input["fail"] != true, "synthetic tool failure");
        Ok(input)
    }
    fn output_to_content_erased(&self, _: &Value, output: &Value) -> anyhow::Result<String> {
        Ok(output.to_string())
    }
    fn add_context(&self, _: &Value, _: &mut TestContext, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

struct Harness {
    state: ActorState<TestContext>,
    actor: ActorRef<Message>,
    calls: Arc<AtomicUsize>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.actor.stop(None);
    }
}

async fn harness() -> Harness {
    let (actor, _) = Actor::spawn(None, IdleActor, ()).await.unwrap();
    let config = ConfigContext::new(Config::OpenAI(OpenAIConfig {
        auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
            api_key: None,
            url: "http://127.0.0.1:1".into(),
        }),
        model: "fixture".into(),
        effort: OpenAIEffort::Low,
        request_encrypted_reasoning: None,
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    let (tui_tx, _) = flume::unbounded();
    let state = ActorState::new(
        Dependency {
            client: LLmClient::new(config).unwrap(),
            tools: vec![Arc::new(EchoTool(calls.clone()))],
            context: TestContext {
                task: Some("Inspect the fixture.".into()),
                revision: 1,
            },
            tui_tx,
            debug_mode: false,
        },
        actor.clone(),
        None,
    )
    .await
    .unwrap();
    Harness {
        state,
        actor,
        calls,
    }
}

async fn consume(
    state: &mut ActorState<TestContext>,
    event: llm::StreamEvent,
) -> anyhow::Result<()> {
    // Exercise the same log format as the runtime, in addition to provider mapping.
    let event = serde_json::from_value(serde_json::to_value(event)?)?;
    match state.stream_processor.process_stream_event(event).await? {
        StreamNextStep::ToolUse => {
            let items = state.stream_processor.extract_and_pre_process()?;
            let results = state.process_tools(items).await;
            state.save_history(results)?;
        }
        StreamNextStep::Done => {
            let items = state.stream_processor.extract_and_pre_process()?;
            let results = state.stream_items_to_res(items).await;
            state.save_history(results)?;
        }
        _ => {}
    }
    Ok(())
}

async fn openai_event(state: &mut ActorState<TestContext>, event: Value) -> anyhow::Result<()> {
    let event: openai::StreamEvent = serde_json::from_value(event)?;
    if let Some(event) = Option::<llm::StreamEvent>::from(event) {
        consume(state, event).await?;
    }
    Ok(())
}

#[tokio::test]
async fn replay_preserves_reasoning_phase_arguments_and_results_across_turns() {
    let mut h = harness().await;
    for line in include_str!("../resources/openai_tool_cycle.jsonl").lines() {
        openai_event(&mut h.state, serde_json::from_str(line).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(h.calls.load(Ordering::SeqCst), 2);
    assert_eq!(h.state.history.len(), 4); // workspace, task, assistant, tool results
    assert_eq!(h.state.history[2].content.len(), 4);
    assert_eq!(h.state.history[3].content.len(), 2);
    let request = openai::ClientRequest::try_from(h.state.build_request()).unwrap();
    assert_eq!(
        request.instructions.as_deref(),
        Some("Follow the fixture's operating instructions.")
    );
    let input = serde_json::to_value(request.input).unwrap();
    assert_eq!(input[0]["content"], "workspace revision 1");
    assert_eq!(input[1]["content"], "Inspect the fixture.");
    assert_eq!(
        input[2],
        json!({
            "type":"reasoning", "id":"rs_1", "summary":[], "encrypted_content":"opaque-state",
            "status":"completed", "content":[],
        })
    );
    assert_eq!(input[3]["phase"], "commentary");
    let arguments: Value = serde_json::from_str(input[4]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(
        arguments,
        json!({
            "filter":{"enabled":true,"range":{"start":2,"end":4}},
            "names":["a","b"],"count":3,"optional":null,
        })
    );
    assert_eq!(input[4]["call_id"], "call_1");
    assert_eq!(input[5]["call_id"], "call_2");
    assert_eq!(input[6]["type"], "function_call_output");
    assert_eq!(input[6]["call_id"], "call_1");
    assert_eq!(input[7]["output"], "synthetic tool failure");
    assert!(matches!(
        &h.state.history[3].content[1],
        llm::ContentBlock::ToolResult {
            is_error: Some(true),
            ..
        }
    ));

    let second_call = json!({
        "type":"function_call", "id":"fc_3", "call_id":"call_3", "name":"echo",
        "arguments":"{\"count\":1}",
    });
    for event in [
        json!({"type":"response.created","response":{"id":"resp_2","model":"fixture"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{
            "type":"reasoning","id":"rs_2","summary":[],"encrypted_content":"next-state"
        }}),
        json!({"type":"response.output_item.done","output_index":1,"item":second_call}),
        json!({"type":"response.completed","response":{"id":"resp_2","status":"completed","output":[second_call]}}),
        json!({"type":"response.created","response":{"id":"resp_3","model":"fixture"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{
            "type":"message","id":"msg_final","phase":"final_answer",
            "content":[{"type":"output_text","text":"Finished."}]
        }}),
        json!({"type":"response.completed","response":{"id":"resp_3","status":"completed"}}),
    ] {
        openai_event(&mut h.state, event).await.unwrap();
    }
    assert_eq!(h.calls.load(Ordering::SeqCst), 3);
    let request = openai::ClientRequest::try_from(h.state.build_request()).unwrap();
    let input = serde_json::to_value(request.input).unwrap();
    let items = input.as_array().unwrap();
    let reasoning: Vec<_> = items.iter().filter(|x| x["type"] == "reasoning").collect();
    assert_eq!(reasoning.len(), 2);
    assert_eq!(reasoning[0]["encrypted_content"], "opaque-state");
    assert_eq!(reasoning[1]["encrypted_content"], "next-state");
    assert_eq!(items.last().unwrap()["phase"], "final_answer");
    assert_eq!(h.state.history.last().unwrap().text(), "Finished.");
    assert!(!h.state.history[2].to_string().contains("opaque-state"));
    assert!(claude::ClientRequest::try_from(h.state.build_request()).is_err());
}

#[tokio::test]
async fn claude_replay_preserves_thinking_signatures_and_typed_tool_input() {
    let mut h = harness().await;
    let arguments = json!({"flag":false,"range":{"start":2},"items":[1,null]});
    for event in [
        json!({"type":"message_start","message":{
            "id":"claude_1","type":"message","role":"assistant","content":[],
            "model":"fixture","stop_reason":null,"stop_sequence":null,
            "usage":{"input_tokens":1,"output_tokens":0}
        }}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"Initial "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thought."}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed-"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"state"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_1","name":"echo","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":arguments.to_string()}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":2}}),
    ] {
        let event: claude::StreamEvent = serde_json::from_value(event).unwrap();
        consume(&mut h.state, event.into()).await.unwrap();
    }
    let request = claude::ClientRequest::try_from(h.state.build_request()).unwrap();
    assert_eq!(
        request.system.as_deref(),
        Some("Follow the fixture's operating instructions.")
    );
    let messages = serde_json::to_value(request.messages).unwrap();
    assert_eq!(
        messages[2]["content"][0],
        json!({"type":"thinking","thinking":"Initial thought.","signature":"signed-state"})
    );
    assert_eq!(messages[2]["content"][1]["input"], arguments);
    assert_eq!(messages[3]["content"][0]["tool_use_id"], "tool_1");
    assert_eq!(h.calls.load(Ordering::SeqCst), 1);
    assert!(openai::ClientRequest::try_from(h.state.build_request()).is_err());
}

#[tokio::test]
async fn clear_reloads_workspace_and_keeps_instructions_without_the_old_task() {
    let mut h = harness().await;
    h.state
        .history
        .push(llm::Message::new_assistant("Old result.".into()));
    h.state.cur_context.revision = 2;
    h.state.clear_history().await;
    let request = h.state.build_request();
    assert_eq!(
        request.system.as_deref(),
        Some("Follow the fixture's operating instructions.")
    );
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].text(), "workspace revision 2");
    assert!(h.state.cur_context.task.is_none());
}

#[tokio::test]
async fn unknown_tool_with_nested_input_is_recorded_as_a_recoverable_result() {
    let mut h = harness().await;
    let call = json!({"type":"function_call","id":"fc_1","call_id":"call_1","name":"missing","arguments":"{\"flag\":false,\"nested\":{\"n\":3}}"});
    for event in [
        json!({"type":"response.created","response":{"id":"resp_1"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":call}),
        json!({"type":"response.completed","response":{"status":"completed","output":[call]}}),
    ] {
        openai_event(&mut h.state, event).await.unwrap();
    }
    let request = openai::ClientRequest::try_from(h.state.build_request()).unwrap();
    let input = serde_json::to_value(request.input).unwrap();
    assert!(
        input[3]["output"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn incomplete_tool_prevents_dispatch_of_the_entire_batch() {
    let mut h = harness().await;
    for event in [
        json!({"type":"response.created","response":{"id":"resp_1"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{
            "type":"function_call","id":"fc_1","call_id":"call_1","name":"echo","arguments":"{}"
        }}),
        json!({"type":"response.output_item.added","output_index":1,"item":{
            "type":"function_call","id":"fc_2","call_id":"call_2","name":"echo"
        }}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"unfinished\":"}),
    ] {
        openai_event(&mut h.state, event).await.unwrap();
    }
    let result = openai_event(
        &mut h.state,
        json!({"type":"response.completed","response":{"status":"completed"}}),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("incomplete"));
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn non_object_tool_arguments_are_rejected_before_dispatch() {
    let mut h = harness().await;
    for event in [
        json!({"type":"response.created","response":{"id":"resp_1"}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{
            "type":"function_call","id":"fc_1","call_id":"call_1","name":"echo","arguments":"[1,2]"
        }}),
    ] {
        openai_event(&mut h.state, event).await.unwrap();
    }
    let result = openai_event(
        &mut h.state,
        json!({"type":"response.completed","response":{"status":"completed"}}),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("JSON object"));
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_or_truncated_responses_never_dispatch_tools() {
    for terminal in ["response.failed", "response.incomplete"] {
        let mut h = harness().await;
        let call = json!({
            "type":"function_call","id":"fc_1","call_id":"call_1",
            "name":"echo","arguments":"{}"
        });
        openai_event(
            &mut h.state,
            json!({"type":"response.created","response":{"id":"resp_1"}}),
        )
        .await
        .unwrap();
        openai_event(
            &mut h.state,
            json!({"type":"response.output_item.done","output_index":0,"item":call}),
        )
        .await
        .unwrap();
        let result = openai_event(
            &mut h.state,
            json!({
                "type":terminal,"response":{
                    "output":[call],"error":{"message":"synthetic provider failure"},
                    "incomplete_details":{"reason":"max_output_tokens"}
                }
            }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(h.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn malformed_completed_arguments_return_an_error_without_dispatch() {
    let mut h = harness().await;
    openai_event(
        &mut h.state,
        json!({"type":"response.created","response":{"id":"resp_1"}}),
    )
    .await
    .unwrap();
    let result = openai_event(
        &mut h.state,
        json!({
            "type":"response.output_item.done","output_index":0,"item":{
                "type":"function_call","id":"fc_1","call_id":"call_1","name":"echo","arguments":"{"
            }
        }),
    )
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("invalid_tool_arguments")
    );
    assert_eq!(h.calls.load(Ordering::SeqCst), 0);
}
