use super::*;
use tools::tool_defs::ToolId;

fn input() -> ContextInput {
    ContextInput {
        runtime: None,
        prompt_cache_key: Some("fixture-session".into()),
        purpose: clients::llm::RequestPurpose::Conversation,
        history: vec![Message::new("workspace".into())],
        checkpoint: Checkpoint::default(),
        instructions: "Current operating instructions".into(),
        tools: vec![],
        limits: ContextLimits::new(12_000, 2048).unwrap(),
        native: NativeCompaction::Disabled,
        mode: RequestMode::Compact,
    }
}

#[test]
fn compaction_trigger_uses_ninety_percent_of_the_context_window() {
    let astra = ContextLimits::new(272_000, 16_000).unwrap();
    assert_eq!(astra.trigger(), 244_800);

    let response_constrained = ContextLimits::new(12_000, 2048).unwrap();
    assert_eq!(response_constrained.trigger(), response_constrained.input());
}

enum ExchangeOutcome {
    Succeeded,
    Failed,
}

fn exchange(
    history: &mut Vec<Message>,
    id: &str,
    name: &str,
    output: &str,
    outcome: ExchangeOutcome,
) {
    let tool_id = ToolId {
        id: id.to_owned().try_into().unwrap(),
        call_id: None,
    };
    history.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolBlock {
            tool_id: tool_id.clone(),
            name: name.to_owned().try_into().unwrap(),
            input: Default::default(),
        }],
    });
    history.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_id,
            content: output.into(),
            is_error: match outcome {
                ExchangeOutcome::Succeeded => None,
                ExchangeOutcome::Failed => Some(true),
            },
        }],
    });
}

#[test]
fn token_budget_does_not_treat_text_or_json_bytes_as_tokens() {
    for text in [
        "The requested change preserves existing behavior.\n",
        "fn main() { println!(\"hello world\"); }\n",
        "こんにちは世界。你好世界。\n",
    ] {
        let mut input = input();
        input.mode = RequestMode::Continue;
        input.history.push(Message::new(text.repeat(300)));
        assert!(serde_json::to_vec(&input.history).unwrap().len() > input.limits.trigger());
        let BudgetPlan::Ready(request) = input.plan().unwrap() else {
            panic!("text within the token budget should not compact")
        };
        assert!(estimated_tokens(&request).unwrap() <= input.limits.trigger());
        assert_eq!(request.messages.last().unwrap().text(), text.repeat(300));
    }
}

#[test]
fn text_prompt_rejects_tools_and_nontext_content() {
    let mut request = ClientRequest::new(vec![Message::new("plain text".into())]);
    assert!(TextPrompt::new(&request).is_ok());
    request.tools.push(ToolDefinition::Search {
        name: "web_search".into(),
    });
    assert!(TextPrompt::new(&request).is_err());
    request.tools.clear();
    exchange(
        &mut request.messages,
        "read",
        "read_file",
        "result",
        ExchangeOutcome::Succeeded,
    );
    for message in request.messages.iter().skip(1) {
        assert!(TextPrompt::new(&ClientRequest::new(vec![message.clone()])).is_err());
    }
    let request = ClientRequest::new(vec![Message {
        role: Role::User,
        content: vec![ContentBlock::RuntimeUpdate(
            clients::runtime_update::RuntimeUpdate::Snapshot(Default::default()),
        )],
    }]);
    assert!(TextPrompt::new(&request).is_err());
}

#[test]
fn repeated_compaction_preserves_requirements_questions_evidence_and_recent_pairs() {
    let mut input = input();
    input.history.push(Message::new(
        "Never modify the public API. Keep existing edits.".into(),
    ));
    exchange(
        &mut input.history,
        "check",
        "cargo_check",
        "Compiler error E0308 in src/main.rs",
        ExchangeOutcome::Failed,
    );
    exchange(
        &mut input.history,
        "test",
        "cargo",
        "Focused regression passed",
        ExchangeOutcome::Succeeded,
    );
    for id in ["one", "two", "three", "four"] {
        exchange(
            &mut input.history,
            id,
            "read_file",
            &"old data ".repeat(180),
            ExchangeOutcome::Succeeded,
        );
    }
    input.runtime = Some(Default::default());
    input
        .runtime
        .as_mut()
        .unwrap()
        .questions
        .push(common_models::interaction::Question {
            purpose: Default::default(),
            choices: Vec::new(),
            allow_free_text: true,
            id: "target".into(),
            prompt: "Which target?".into(),
            required: true,
        });
    for generation in 1..=3 {
        let BudgetPlan::Compact(plan) = input.plan().unwrap() else {
            panic!("compaction expected")
        };
        let recent = serde_json::to_value(&input.history[plan.through..]).unwrap();
        input.checkpoint = input
            .compacted(
                &plan,
                Memory::Summary(
                    "Investigated the bug; implementation and verification remain pending.".into(),
                ),
            )
            .unwrap();
        let request = input.request(&input.checkpoint).unwrap();
        let serialized = serde_json::to_string(&request.messages).unwrap();
        assert!(serialized.contains("Never modify the public API. Keep existing edits."));
        assert!(serialized.contains("Compiler error E0308 in src/main.rs"));
        assert!(serialized.contains("Focused regression passed"));
        assert!(serialized.contains("Which target?"));
        assert!(serialized.contains("remain pending"));
        assert_eq!(input.checkpoint.generation, generation);
        let tail = request
            .messages
            .iter()
            .filter(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::ToolBlock { .. } | ContentBlock::ToolResult { .. }
                    )
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(serde_json::to_value(tail).unwrap(), recent);
        CompleteHistory::new(&request.messages).unwrap();
        assert!(
            estimated_tokens(&request).unwrap() + (input.limits.response() as usize)
                < input.limits.ceiling()
        );
        for id in [
            format!("next-{generation}-a"),
            format!("next-{generation}-b"),
        ] {
            exchange(
                &mut input.history,
                &id,
                "read_file",
                &"more data ".repeat(180),
                ExchangeOutcome::Succeeded,
            );
        }
    }
}

#[test]
fn requests_budget_optional_workspace_after_instructions_and_latest_user_input() {
    let mut input = input();
    input.mode = RequestMode::Continue;
    input.history[0] = Message::new("symbol ".repeat(100_000));
    input
        .history
        .push(Message::new("Keep this exact constraint".into()));
    let BudgetPlan::Ready(request) = input.plan().unwrap() else {
        panic!("unexpected compaction")
    };
    assert!(estimated_tokens(&request).unwrap() <= input.limits.trigger());
    assert_eq!(request.system.as_deref(), Some(input.instructions.as_str()));
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text() == "Keep this exact constraint")
    );
    assert!(request.messages[0].text().contains("bytes omitted"));
    input.instructions = "mandatory instruction ".repeat(10_000);
    assert!(input.plan().is_err());
}

#[test]
fn compaction_requires_complete_tool_exchanges_without_duplicates_or_interruptions() {
    let mut input = input();
    exchange(
        &mut input.history,
        "one",
        "read_file",
        "first",
        ExchangeOutcome::Succeeded,
    );
    exchange(
        &mut input.history,
        "two",
        "read_file",
        "second",
        ExchangeOutcome::Succeeded,
    );
    let calls = Message {
        role: Role::Assistant,
        content: input.history[1]
            .content
            .iter()
            .chain(&input.history[3].content)
            .cloned()
            .collect(),
    };
    let mut pending = vec![input.history[0].clone(), calls, input.history[2].clone()];
    assert!(CompleteHistory::new(&pending[..2]).is_err());
    assert!(CompleteHistory::new(&pending).is_err());
    let mut duplicate = pending.clone();
    duplicate.push(input.history[1].clone());
    assert!(CompleteHistory::new(&duplicate).is_err());
    let mut interrupted = pending.clone();
    interrupted.push(Message::new("interrupting request".into()));
    interrupted.push(input.history[4].clone());
    assert!(CompleteHistory::new(&interrupted).is_err());
    pending.push(input.history[4].clone());
    assert_eq!(CompleteHistory::new(&pending).unwrap().ends, vec![4]);
    for through in [2, 3] {
        assert!(Checkpoint::new(&pending, through, 1, Memory::Summary("summary".into())).is_err());
    }
    pending.push(input.history[2].clone());
    assert!(CompleteHistory::new(&pending).is_err());
}

#[test]
fn summary_cannot_discard_irreducible_requirements_or_exceed_the_budget() {
    let mut input = input();
    for _ in 0..4 {
        input
            .history
            .push(Message::new("mandatory requirement ".repeat(1500)));
        input
            .history
            .push(Message::new_assistant("acknowledged".into()));
    }
    let checkpoint =
        Checkpoint::new(&input.history, 5, 1, Memory::Summary("short".into())).unwrap();
    let request = input.request(&checkpoint).unwrap();
    assert!(estimated_tokens(&request).unwrap() > input.limits.input());
    assert!(
        input
            .compacted(
                &CompactionPlan {
                    through: 5,
                    request
                },
                Memory::Summary("short".into())
            )
            .is_err()
    );
}

#[test]
fn runtime_deltas_preserve_prefixes_and_compaction_resets_the_snapshot() {
    use clients::runtime_update::{RuntimeSnapshot, RuntimeUpdate};
    let mut input = input();
    input.mode = RequestMode::Continue;
    input.history[0] = Message::new("workspace symbol ".repeat(10_000));
    input
        .history
        .push(Message::new("Keep this requirement".into()));
    input.runtime = Some(RuntimeSnapshot::default());
    let mut previous = input.request(&input.checkpoint).unwrap();
    input
        .history
        .extend(input.runtime_update(&input.checkpoint));
    for index in 0..4 {
        exchange(
            &mut input.history,
            &format!("read-{index}"),
            "read_file",
            &"source ".repeat(300),
            ExchangeOutcome::Succeeded,
        );
        let state = input.runtime.as_mut().unwrap();
        state
            .evidence
            .insert(format!("tool:read-{index}"), "Inspected source".into());
        state.workers = vec![format!("worker: cycle {index}")];
        let request = input.request(&input.checkpoint).unwrap();
        assert_eq!(
            serde_json::to_value(&request.messages[..previous.messages.len()]).unwrap(),
            serde_json::to_value(&previous.messages).unwrap()
        );
        assert_eq!(request.system, previous.system);
        assert_eq!(request.prompt_cache_key, previous.prompt_cache_key);
        let update = input.runtime_update(&input.checkpoint).unwrap();
        assert!(
            matches!(&update.content[0], ContentBlock::RuntimeUpdate(RuntimeUpdate::Changes(changes)) if changes.planning.is_none() && changes.questions.is_none() && changes.evidence.len() == 1)
        );
        input.history.push(update);
        assert!(input.runtime_update(&input.checkpoint).is_none());
        previous = request;
    }
    let stored = serde_json::to_vec(&input.history).unwrap();
    input.history = serde_json::from_slice(&stored).unwrap();
    assert_eq!(
        serde_json::to_value(input.request(&input.checkpoint).unwrap().messages).unwrap(),
        serde_json::to_value(&previous.messages).unwrap()
    );
    input.mode = RequestMode::Compact;
    let BudgetPlan::Compact(plan) = input.plan().unwrap() else {
        panic!("expected compaction")
    };
    input.checkpoint = input
        .compacted(&plan, Memory::Summary("Earlier files inspected".into()))
        .unwrap();
    let snapshot = input.runtime_update(&input.checkpoint).unwrap();
    assert!(
        matches!(&snapshot.content[0], ContentBlock::RuntimeUpdate(RuntimeUpdate::Snapshot(state)) if Some(state) == input.runtime.as_ref())
    );
    let request = input.request(&input.checkpoint).unwrap();
    let updates = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter(|block| matches!(block, ContentBlock::RuntimeUpdate(_)))
        .count();
    assert_eq!(updates, 1);
    assert_eq!(
        protected(&input.history, input.checkpoint.through).unwrap()[0].text(),
        "Keep this requirement"
    );
    assert!(
        protected(&input.history, input.checkpoint.through)
            .unwrap()
            .iter()
            .all(|message| !message.to_string().contains("Runtime state"))
    );
    input.history.push(snapshot);
    let checkpoint = serde_json::to_vec(&input.checkpoint).unwrap();
    input.checkpoint = serde_json::from_slice(&checkpoint).unwrap();
    assert!(input.runtime_update(&input.checkpoint).is_none());
    assert_eq!(
        serde_json::to_value(input.request(&input.checkpoint).unwrap().messages).unwrap(),
        serde_json::to_value(request.messages).unwrap()
    );
    let state = input.runtime.as_mut().unwrap();
    state.evidence.remove("tool:read-0");
    state.workers.clear();
    let update = input.runtime_update(&input.checkpoint).unwrap();
    assert!(
        matches!(&update.content[0], ContentBlock::RuntimeUpdate(RuntimeUpdate::Changes(changes)) if changes.evidence.get("tool:read-0") == Some(&None) && changes.workers == Some(vec![]))
    );
}

#[test]
fn legacy_checkpoints_default_the_runtime_boundary() {
    let checkpoint: Checkpoint =
        serde_json::from_value(serde_json::json!({"through":1,"generation":0,"memory":null}))
            .unwrap();
    assert_eq!(checkpoint.runtime_from, 0);
}
