use super::*;

#[test]
fn session_commands_accept_only_their_expected_arguments() {
    assert_eq!(Command::parse("diff"), Ok(Command::Diff));
    assert_eq!(
        Command::parse("undo joe-edit"),
        Ok(Command::Undo("joe-edit".into()))
    );
    assert!(Command::parse("undo").is_err());
    assert!(Command::parse("diff extra").is_err());
    assert_eq!(Command::parse("sessions"), Ok(Command::Sessions));
    assert_eq!(Command::parse("fork"), Ok(Command::Fork));
    assert_eq!(Command::parse("compact"), Ok(Command::Compact));
    assert!(Command::parse("fork workspace").is_err());
    assert!(Command::parse("compact extra").is_err());
    assert_eq!(
        Command::parse("resume saved-session"),
        Ok(Command::Resume(ResumeTarget::Session {
            id: "saved-session".into()
        }))
    );
    assert_eq!(Command::parse(" new "), Ok(Command::New));
    assert_eq!(
        Command::parse("resume"),
        Ok(Command::Resume(ResumeTarget::Picker))
    );
    assert!(Command::parse("resume one two").is_err());
    assert!(Command::parse("clear extra").is_err());
    assert_eq!(Command::parse("context"), Ok(Command::PrintContext));
}

#[test]
fn interaction_commands_preserve_text_and_require_explicit_answer_types() {
    assert_eq!(Command::parse("plan"), Ok(Command::Plan));
    assert_eq!(Command::parse("implement"), Ok(Command::Implement));
    assert_eq!(Command::parse("questions"), Ok(Command::Questions));
    assert_eq!(
        Command::parse("answer target choice lib"),
        Ok(Command::Answer(QuestionAnswer {
            id: "target".into(),
            answer: Answer::Choice {
                choice_id: "lib".into()
            }
        }))
    );
    assert_eq!(
        Command::parse("answer target text keep  spacing\nand newlines"),
        Ok(Command::Answer(QuestionAnswer {
            id: "target".into(),
            answer: Answer::Text("keep  spacing\nand newlines".into())
        }))
    );
    assert_eq!(
        Command::parse("steer preserve  the API\nand tests"),
        Ok(Command::Steer("preserve  the API\nand tests".into()))
    );
    for input in [
        "plan extra",
        "implement extra",
        "answer",
        "answer target",
        "answer target lib",
        "answer target choice lib extra",
        "answer target text",
        "steer",
    ] {
        assert!(Command::parse(input).is_err(), "{input}");
    }
}
