use super::{Action, AskImmutableWorkerInput};
use serde_json::{Value, json};

fn parse_action(value: Value) -> anyhow::Result<Action> {
    Action::try_from(serde_json::from_value::<AskImmutableWorkerInput>(value)?)
}

#[test]
fn list_accepts_omitted_or_null_parameters() {
    for input in [
        json!({"action": "list"}),
        json!({"action": "list", "worker_id": null, "question": null}),
    ] {
        assert_eq!(parse_action(input).unwrap(), Action::List);
    }
}

#[test]
fn list_rejects_question_parameters() {
    for input in [
        json!({"action": "list", "worker_id": "reference"}),
        json!({"action": "list", "question": "What is recorded?"}),
        json!({"action": "list", "worker_id": "", "question": ""}),
    ] {
        assert!(parse_action(input).is_err());
    }
}

#[test]
fn ask_preserves_worker_id_and_question() {
    let worker_id = " reference ";
    let question = "  What is recorded?\n";
    assert_eq!(
        parse_action(json!({"action": "ask", "worker_id": worker_id, "question": question}))
            .unwrap(),
        Action::Ask {
            worker_id: worker_id.into(),
            question: question.into(),
        }
    );
}

#[test]
fn invalid_actions_and_incomplete_questions_are_rejected() {
    for input in [
        json!({"action": "unknown"}),
        json!({"action": ""}),
        json!({"action": "ask"}),
        json!({"action": "ask", "worker_id": "reference"}),
        json!({"action": "ask", "question": "What is recorded?"}),
        json!({"action": "ask", "worker_id": null, "question": "What is recorded?"}),
        json!({"action": "ask", "worker_id": "reference", "question": null}),
        json!({"action": "ask", "worker_id": "", "question": "What is recorded?"}),
        json!({"action": "ask", "worker_id": " \t\n", "question": "What is recorded?"}),
        json!({"action": "ask", "worker_id": "reference", "question": ""}),
        json!({"action": "ask", "worker_id": "reference", "question": " \t\n\u{2003}"}),
    ] {
        assert!(parse_action(input).is_err());
    }
}

#[test]
fn question_limit_counts_utf8_bytes() {
    for question in ["x".repeat(16384), "é".repeat(8192)] {
        assert_eq!(
            parse_action(json!({"action": "ask", "worker_id": "reference", "question": question}))
                .unwrap(),
            Action::Ask {
                worker_id: "reference".into(),
                question,
            }
        );
    }
    for question in [
        "x".repeat(16385),
        format!("{}é", "x".repeat(16383)),
        format!(" {}", "é".repeat(8192)),
    ] {
        assert!(
            parse_action(json!({"action": "ask", "worker_id": "reference", "question": question}))
                .is_err()
        );
    }
}
