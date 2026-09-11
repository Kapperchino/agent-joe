use super::*;

fn formatter() -> MessageFormatter {
    MessageFormatter::new(80)
}

#[test]
fn empty_stream_does_not_render_or_commit_blank_lines() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.start_stream(true, &formatter);

    assert_eq!(transcript.active_lines(&formatter), None);

    transcript.finish_stream(true, &formatter);

    assert!(transcript.committed_lines().is_empty());
}

#[test]
fn stream_boundaries_do_not_create_duplicate_blank_lines() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Message("user".to_string()), &formatter);
    transcript.start_stream(true, &formatter);
    transcript.push_stream_chunk("assistant");
    transcript.finish_stream(true, &formatter);
    transcript.start_stream(true, &formatter);
    transcript.push_stream_chunk("next");
    transcript.finish_stream(true, &formatter);

    assert_eq!(
        transcript.committed_lines(),
        &[
            "user".to_string(),
            String::new(),
            "assistant".to_string(),
            String::new(),
            "next".to_string(),
            String::new(),
        ]
    );
}
