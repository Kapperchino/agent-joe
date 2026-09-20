use super::*;
use common_models::interaction::QuestionGate;

fn question() -> Question {
    Question {
        id: "target".into(),
        purpose: QuestionPurpose::Clarification,
        prompt: "Which target?".into(),
        required: true,
        choices: Vec::new(),
        allow_free_text: true,
    }
}

#[test]
fn rejected_question_commit_preserves_the_current_gate() {
    let state = InteractionState::default();
    let update = state.asked(question()).unwrap();
    let result = update.commit(|event| match event {
        InteractionEvent::QuestionAsked(question) if question.id == "target" => {
            Err(anyhow::anyhow!("Storage unavailable"))
        }
        _ => panic!("Expected the question event"),
    });
    assert!(result.is_err());
    assert_eq!(state.questions().gate(), QuestionGate::Open);
    assert!(state.questions().pending().is_empty());
}

#[test]
fn invalid_answers_preserve_the_required_question() {
    let state = InteractionState::default()
        .asked(question())
        .unwrap()
        .commit(|_| Ok(()))
        .unwrap();
    let result = state.answered(
        "missing",
        Answer::Choice {
            choice_id: "missing".into(),
        },
    );
    assert!(result.is_err());
    assert_eq!(state.questions().gate(), QuestionGate::Required);
    assert_eq!(state.questions().pending()[0].id, "target");
}
