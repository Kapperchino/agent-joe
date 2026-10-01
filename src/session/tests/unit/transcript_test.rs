use super::*;
use analysis::contexts::rust_context::RustContext;
use clients::llm::{ContentBlock, Message, Role};
use serde_json::{Value, json};
use std::sync::Arc;
use tools::tool_defs::{ErasedTool, ToolId};

fn tools() -> Vec<ErasedToolRef<RustContext, ()>> {
    vec![
        Arc::new(ErasedTool::<tools::read_file::ReadFile, RustContext, ()>::new()),
        Arc::new(ErasedTool::<tools::grep::GrepTool, RustContext, ()>::new()),
        Arc::new(ErasedTool::<tools::apply_patch::ApplyPatch, RustContext, ()>::new()),
    ]
}

fn tool_id(id: &str) -> ToolId {
    ToolId {
        id: id.to_owned().try_into().unwrap(),
        call_id: None,
    }
}

fn call(name: &str, id: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolBlock {
        tool_id: tool_id(id),
        name: name.to_owned().try_into().unwrap(),
        input: input.as_object().unwrap().clone(),
    }
}

#[test]
fn resumed_transcript_uses_live_summaries_without_raw_inputs_or_results() {
    let tools = tools();
    let conversation = Conversation::new(
        vec![
            Message::new("Workspace context".into()),
            Message::new("Find and fix the bug".into()),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ThinkingBlock {
                        thinking: "Check the source first".into(),
                        signature: "private signature".into(),
                        reasoning_id: None,
                    },
                    call("read_file", "read", json!({"file_path": "src/lib.rs"})),
                    call(
                        "grep",
                        "search",
                        json!({
                            "regex": "bug",
                            "add_start": 1,
                            "add_end": 2,
                            "include": "raw input only"
                        }),
                    ),
                ],
            },
            Message {
                role: Role::User,
                content: vec![
                    ContentBlock::ToolResult {
                        tool_id: tool_id("search"),
                        content: json!({"matches": "huge raw result\n".repeat(10_000)}).to_string(),
                        is_error: None,
                    },
                    ContentBlock::ToolResult {
                        tool_id: tool_id("read"),
                        content: "raw failure details".repeat(10_000),
                        is_error: Some(true),
                    },
                ],
            },
            Message::new_assistant("**Fixed** the bug".into()),
        ],
        None,
    );
    let before = serde_json::to_value(conversation.history()).unwrap();
    let transcript = session_transcript(&conversation, "saved", &tools);

    assert_eq!(transcript.id, "saved");
    assert_eq!(
        transcript.messages,
        vec![
            SessionMessage::User("Find and fix the bug".into()),
            SessionMessage::Thinking("Check the source first".into()),
            SessionMessage::Tool("- read `src/lib.rs`".into()),
            SessionMessage::Tool("- grep `bug` (before: 1, after: 2)".into()),
            SessionMessage::Assistant("**Fixed** the bug".into()),
        ]
    );
    assert_eq!(
        serde_json::to_value(conversation.history()).unwrap(),
        before
    );
}

#[test]
fn resumed_transcript_falls_back_to_tool_names_for_old_or_invalid_calls() {
    let conversation = Conversation::new(
        vec![
            Message::new("Workspace context".into()),
            Message {
                role: Role::Assistant,
                content: vec![
                    call(
                        "old_tool",
                        "unknown",
                        json!({"body": "large input".repeat(10_000)}),
                    ),
                    call(
                        "read_file",
                        "invalid",
                        json!({"file_path": {"invalid": true}}),
                    ),
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_id: tool_id("orphaned"),
                    content: "Result without a saved call".into(),
                    is_error: None,
                }],
            },
        ],
        None,
    );

    assert_eq!(
        session_transcript(&conversation, "old", &tools()).messages,
        vec![
            SessionMessage::Tool("- old_tool".into()),
            SessionMessage::Tool("- read_file".into()),
        ]
    );
}

#[test]
fn resumed_transcript_keeps_patch_display_details_without_result_json() {
    let tools = tools();
    let input = json!({
        "patch": "*** Begin Patch\n*** Add File: never-created.rs\n+fn restored() {}\n*** End Patch"
    });
    let display = tools[2].display_erased(&input).unwrap();
    let conversation = Conversation::new(
        vec![
            Message::new("Workspace context".into()),
            Message {
                role: Role::Assistant,
                content: vec![call("apply_patch", "patch", input)],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_id: tool_id("patch"),
                    content: json!({"status": "ok", "edit": {"id": "saved edit"}}).to_string(),
                    is_error: None,
                }],
            },
        ],
        None,
    );

    assert!(display.contains("```diff"));
    assert_eq!(
        session_transcript(&conversation, "patch", &tools).messages,
        vec![SessionMessage::Tool(display)]
    );
}

#[test]
fn resumed_transcript_omits_runtime_updates_and_empty_reasoning() {
    let conversation = Conversation::new(
        vec![
            Message::new("Workspace context".into()),
            Message {
                role: Role::Assistant,
                content: vec![
                    call("read_file", "first", json!({"file_path": "first.rs"})),
                    ContentBlock::OpenAIReasoning(clients::openai::ReasoningItem {
                        id: "hidden-reasoning".into(),
                        summary: Vec::new(),
                        encrypted_content: Some("encrypted content".into()),
                        extra: Default::default(),
                    }),
                    ContentBlock::ThinkingBlock {
                        thinking: " \n".into(),
                        signature: "private".into(),
                        reasoning_id: None,
                    },
                    ContentBlock::MessageBlock {
                        text: String::new(),
                        phase: None,
                    },
                    call("read_file", "second", json!({"file_path": "second.rs"})),
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::RuntimeUpdate(
                    clients::runtime_update::RuntimeUpdate::Snapshot(Default::default()),
                )],
            },
        ],
        None,
    );

    assert_eq!(
        session_transcript(&conversation, "reasoning", &tools()).messages,
        vec![
            SessionMessage::Tool("- read `first.rs`".into()),
            SessionMessage::Tool("- read `second.rs`".into()),
        ]
    );
}
