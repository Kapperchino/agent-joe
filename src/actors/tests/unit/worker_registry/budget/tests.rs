use super::*;
use clients::llm::{
    ClientRequest, Message, MessageDeltaContent, Role, StopReason, StreamEvent, StreamMessage,
    StreamUsage, UsageDelta,
};

fn usage(input_tokens: u32, output_tokens: u32, stop_reason: Option<StopReason>) -> StreamEvent {
    StreamEvent::MessageDelta {
        delta: MessageDeltaContent { stop_reason },
        usage: UsageDelta {
            input_tokens,
            output_tokens,
            ..Default::default()
        },
    }
}

#[test]
fn worker_reservations_preserve_inherited_response_limits() {
    for output_limit in [2048, 16_000, 32_000, 1_000_000] {
        let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
        let request = ClientRequest::new(vec![Message::new("Inspect the files".into())])
            .with_output_limit(output_limit);
        let input = crate::context::estimated_tokens(&request).unwrap();
        budget.reserve(&request).unwrap();
        assert_eq!(request.max_output_tokens, Some(output_limit));
        assert_eq!(
            budget.usage().reserved_tokens,
            input + output_limit as usize
        );
    }
}

#[test]
fn completed_requests_release_unused_tokens_for_later_tool_rounds() {
    let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
    for round in 1..=5 {
        let request = ClientRequest::new(vec![Message::new("Inspect the files".into())])
            .with_output_limit(16_000);
        budget.reserve(&request).unwrap();
        budget
            .observe(&usage(600, 100, Some(StopReason::ToolUse)))
            .unwrap();
        assert_eq!(budget.usage().reserved_tokens, round * 700);
        assert_eq!(budget.usage().reported_input_tokens, round * 600);
        assert_eq!(budget.usage().reported_output_tokens, round * 100);
        assert_eq!(budget.usage().requests, round);
        budget.tool_call().unwrap();
    }
    assert_eq!(budget.usage().state, BudgetState::Available);
}

#[test]
fn cumulative_usage_counts_cached_input_and_output_once_per_request() {
    let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
    let request = ClientRequest::new(vec![Message::new("Inspect the files".into())]);
    budget.reserve(&request).unwrap();
    budget
        .observe(&StreamEvent::MessageStart {
            message: StreamMessage {
                id: "response".into(),
                model: "fixture".into(),
                role: Role::Assistant,
                usage: StreamUsage {
                    input_tokens: 100,
                    cache_creation_input_tokens: 200,
                    cache_read_input_tokens: 300,
                    output_tokens: 1,
                },
            },
        })
        .unwrap();
    budget.observe(&usage(0, 50, None)).unwrap();
    budget.observe(&usage(0, 100, None)).unwrap();
    budget
        .observe(&usage(0, 100, Some(StopReason::EndTurn)))
        .unwrap();
    budget.observe(&StreamEvent::MessageStop).unwrap();
    assert_eq!(budget.usage().reported_input_tokens, 600);
    assert_eq!(budget.usage().reported_output_tokens, 100);
    assert_eq!(budget.usage().reserved_tokens, 700);
}

#[test]
fn openai_usage_subtotals_are_already_included_in_reported_totals() {
    let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
    budget
        .reserve(&ClientRequest::new(vec![Message::new(
            "Inspect files".into(),
        )]))
        .unwrap();
    budget
        .observe(&StreamEvent::MessageDelta {
            delta: clients::llm::MessageDeltaContent {
                stop_reason: Some(StopReason::EndTurn),
            },
            usage: UsageDelta {
                input_tokens: 600,
                output_tokens: 100,
                cached_input_tokens: 550,
                reasoning_tokens: 80,
            },
        })
        .unwrap();
    assert_eq!(budget.usage().reported_input_tokens, 600);
    assert_eq!(budget.usage().reported_output_tokens, 100);
    assert_eq!(budget.usage().reserved_tokens, 700);
}

#[test]
fn incomplete_or_unreported_requests_keep_their_reservations() {
    for event in [
        usage(600, 100, None),
        usage(0, 0, Some(StopReason::EndTurn)),
    ] {
        let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
        let request = ClientRequest::new(vec![Message::new("Inspect the files".into())])
            .with_output_limit(16_000);
        budget.reserve(&request).unwrap();
        let reserved = budget.usage().reserved_tokens;
        budget.observe(&event).unwrap();
        assert_eq!(budget.usage().reserved_tokens, reserved);
        budget.reserve(&request).unwrap();
        assert_eq!(budget.usage().reserved_tokens, reserved * 2);
        assert_eq!(budget.usage().state, BudgetState::Available);
    }
}

#[test]
fn output_only_usage_preserves_the_input_estimate_and_explicit_output_limit() {
    let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
    let request =
        ClientRequest::new(vec![Message::new("Inspect the files".into())]).with_output_limit(512);
    let input = crate::context::estimated_tokens(&request).unwrap();
    budget.reserve(&request).unwrap();
    assert_eq!(request.max_output_tokens, Some(512));
    budget
        .observe(&usage(0, 100, Some(StopReason::EndTurn)))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, input + 100);
}

#[test]
fn reported_tokens_are_tracked_without_limiting_later_requests() {
    let budget = WorkerBudget::new(BudgetLimits::new(30, 8).unwrap());
    let request =
        ClientRequest::new(vec![Message::new("Inspect the files".into())]).with_output_limit(512);
    for round in 1..=5 {
        budget.reserve(&request).unwrap();
        budget
            .observe(&usage(750_000, 100, Some(StopReason::EndTurn)))
            .unwrap();
        budget.tool_call().unwrap();
        assert_eq!(budget.usage().reserved_tokens, round * 750_100);
        assert_eq!(budget.usage().reported_input_tokens, round * 750_000);
        assert_eq!(budget.usage().reported_output_tokens, round * 100);
        assert_eq!(budget.usage().state, BudgetState::Available);
    }
}

#[test]
fn exhausted_budgets_reject_further_requests_events_and_tools() {
    let budget = WorkerBudget::new(BudgetLimits::new(1, 1).unwrap());
    let request = ClientRequest::new(vec![Message::new("Bounded request".into())]);
    budget.reserve(&request).unwrap();
    assert!(
        budget
            .reserve(&request)
            .unwrap_err()
            .to_string()
            .contains("Worker request budget exhausted (1/1 requests)")
    );
    assert!(budget.tool_call().is_err());
    assert!(budget.observe(&StreamEvent::Ping).is_err());
    assert_eq!(budget.usage().requests, 1);
    assert_eq!(budget.usage().tool_calls, 0);
    assert_eq!(budget.usage().state, BudgetState::Exhausted);
    let encoded = serde_json::to_value(budget.usage()).unwrap();
    assert_eq!(encoded["exhausted"], true);
    let restored: BudgetUsage = serde_json::from_value(encoded).unwrap();
    assert_eq!(restored.state, BudgetState::Exhausted);

    let budget = WorkerBudget::new(BudgetLimits::new(1, 1).unwrap());
    for _ in 0..128 {
        budget.tool_call().unwrap();
    }
    assert!(budget.tool_call().is_err());
    assert!(budget.reserve(&request).is_err());
    assert_eq!(budget.usage().tool_calls, 128);
    assert_eq!(budget.usage().state, BudgetState::Exhausted);
}
