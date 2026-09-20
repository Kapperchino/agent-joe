use super::*;

#[test]
fn completed_usage_releases_reservations_and_ignores_late_updates() {
    let budget = WorkerBudget::default();
    budget
        .reserve(RequestReservation {
            estimated_input_tokens: 600,
            output_tokens: 16000,
        })
        .unwrap();
    budget
        .observe(UsageUpdate::Progress(TokenUsage {
            input_tokens: 500,
            output_tokens: 25,
        }))
        .unwrap();
    budget
        .observe(UsageUpdate::Completed(TokenUsage {
            input_tokens: 500,
            output_tokens: 100,
        }))
        .unwrap();
    budget
        .observe(UsageUpdate::Completed(TokenUsage {
            input_tokens: 1000,
            output_tokens: 200,
        }))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, 600);
    assert_eq!(budget.usage().reported_input_tokens, 500);
    assert_eq!(budget.usage().reported_output_tokens, 100);
    budget
        .reserve(RequestReservation {
            estimated_input_tokens: 1000,
            output_tokens: 400,
        })
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, 2000);
}

#[test]
fn missing_usage_preserves_the_reservation_and_partial_usage_preserves_the_input_estimate() {
    let budget = WorkerBudget::default();
    let reservation = RequestReservation {
        estimated_input_tokens: 600,
        output_tokens: 16000,
    };
    budget.reserve(reservation).unwrap();
    budget
        .observe(UsageUpdate::Completed(TokenUsage::default()))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, 16600);
    budget.reserve(reservation).unwrap();
    budget
        .observe(UsageUpdate::Completed(TokenUsage {
            input_tokens: 0,
            output_tokens: 100,
        }))
        .unwrap();
    assert_eq!(budget.usage().reserved_tokens, 17300);
}

#[test]
fn exhausted_budgets_reject_all_work_and_preserve_the_storage_format() {
    let budget = WorkerBudget::default();
    for _ in 0..128 {
        budget.tool_call().unwrap();
    }
    assert!(budget.tool_call().is_err());
    assert!(
        budget
            .reserve(RequestReservation {
                estimated_input_tokens: 0,
                output_tokens: 0
            })
            .is_err()
    );
    assert!(budget.observe(UsageUpdate::Unreported).is_err());
    let saved = serde_json::to_value(budget.usage()).unwrap();
    assert_eq!(saved["exhausted"], true);
    let restored: BudgetUsage = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.state, BudgetState::Exhausted);
}
