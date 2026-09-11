use super::*;
use clients::llm::{ClientRequest, Message, MessageDeltaContent, StreamEvent, UsageDelta};

#[test]
fn exhausted_budgets_reject_further_requests_events_and_tools() {
    let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
    let mut request = ClientRequest::new(vec![Message::new("Bounded request".into())]);
    budget.reserve(&mut request).unwrap();
    assert!(budget.reserve(&mut request).is_err());
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
