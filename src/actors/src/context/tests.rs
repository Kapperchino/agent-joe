use super::*;
use tools::tool_defs::ToolId;

fn input() -> ContextInput {
    ContextInput {
        history: vec![Message::new("workspace".into())],
        checkpoint: Checkpoint::default(),
        questions: vec![],
        instructions: "Current operating instructions".into(),
        tools: vec![],
        limits: ContextLimits::new(12_000, 2048).unwrap(),
        native: NativeCompaction::Disabled,
        mode: RequestMode::Compact,
    }
}

fn exchange(history: &mut Vec<Message>, id: &str, name: &str, output: &str, is_error: bool) {
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
            is_error: is_error.then_some(true),
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
        true,
    );
    for id in ["one", "two", "three", "four"] {
        exchange(
            &mut input.history,
            id,
            "read_file",
            &"old data ".repeat(180),
            false,
        );
    }
    input.questions.push(crate::session::PendingQuestion {
        id: "target".to_owned().try_into().unwrap(),
        prompt: "Which target?".to_owned().try_into().unwrap(),
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
                false,
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
    input.instructions = "mandatory instruction ".repeat(4000);
    assert!(input.plan().is_err());
}

#[test]
fn compaction_requires_complete_tool_exchanges_without_duplicates_or_interruptions() {
    let mut input = input();
    exchange(&mut input.history, "one", "read_file", "first", false);
    exchange(&mut input.history, "two", "read_file", "second", false);
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
