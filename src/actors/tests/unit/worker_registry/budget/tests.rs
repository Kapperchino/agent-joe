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
        },
    }
}

#[test]
fn completed_requests_release_unused_tokens_for_later_tool_rounds() {
    let budget = WorkerBudget::new(BudgetLimits::new(6000, 30, 8).unwrap());
    for round in 1..=5 {
        let mut request = ClientRequest::new(vec![Message::new("Inspect the files".into())]);
        budget.reserve(&mut request).unwrap();
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
    let budget = WorkerBudget::new(BudgetLimits::new(6000, 30, 8).unwrap());
    let mut request = ClientRequest::new(vec![Message::new("Inspect the files".into())]);
    budget.reserve(&mut request).unwrap();
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
fn incomplete_or_unreported_requests_keep_their_reservations() {
    for event in [
        usage(600, 100, None),
        usage(0, 0, Some(StopReason::EndTurn)),
    ] {
        let budget = WorkerBudget::new(BudgetLimits::new(6000, 30, 8).unwrap());
        let mut request = ClientRequest::new(vec![Message::new("Inspect the files".into())]);
        budget.reserve(&mut request).unwrap();
        let reserved = budget.usage().reserved_tokens;
        budget.observe(&event).unwrap();
        assert_eq!(budget.usage().reserved_tokens, reserved);
        let error = budget.reserve(&mut request).unwrap_err().to_string();
        assert!(error.contains("Worker token budget exhausted"), "{error}");
        assert!(error.contains("next request needs"), "{error}");
    }
}

#[test]
fn output_only_usage_preserves_the_input_estimate_and_explicit_output_limit() {
    let budget = WorkerBudget::new(BudgetLimits::new(6000, 30, 8).unwrap());
    let mut request =
        ClientRequest::new(vec![Message::new("Inspect the files".into())]).with_output_limit(512);
    let input = crate::context::estimated_tokens(&request).unwrap();
    budget.reserve(&mut request).unwrap();
    assert_eq!(request.max_output_tokens, Some(512));
    budget
        .observe(&usage(0, 100, Some(StopReason::EndTurn)))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, input + 100);
}

#[test]
fn actual_usage_above_the_reservation_is_charged_before_the_next_request() {
    let budget = WorkerBudget::new(BudgetLimits::new(6000, 30, 8).unwrap());
    let mut request =
        ClientRequest::new(vec![Message::new("Inspect the files".into())]).with_output_limit(512);
    budget.reserve(&mut request).unwrap();
    budget
        .observe(&usage(5000, 100, Some(StopReason::EndTurn)))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, 5100);
    assert!(budget.reserve(&mut request).is_err());
}

#[test]
fn exhausted_budgets_reject_further_requests_events_and_tools() {
    let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
    let mut request = ClientRequest::new(vec![Message::new("Bounded request".into())]);
    budget.reserve(&mut request).unwrap();
    assert!(
        budget
            .reserve(&mut request)
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

    let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
    budget.reserve(&mut request).unwrap();
    let over_limit = StreamEvent::MessageDelta {
        delta: MessageDeltaContent { stop_reason: None },
        usage: UsageDelta {
            input_tokens: 4096,
            output_tokens: 1,
        },
    };
    assert!(budget.observe(&over_limit).is_err());
    assert!(budget.tool_call().is_err());
    assert!(budget.reserve(&mut request).is_err());
    assert_eq!(budget.usage().reported_output_tokens, 1);
    assert_eq!(budget.usage().state, BudgetState::Exhausted);

    let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
    for _ in 0..128 {
        budget.tool_call().unwrap();
    }
    assert!(budget.tool_call().is_err());
    assert!(budget.reserve(&mut request).is_err());
    assert_eq!(budget.usage().tool_calls, 128);
    assert_eq!(budget.usage().state, BudgetState::Exhausted);
}
