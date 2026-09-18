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

#[test]
fn consecutive_tools_share_one_summary_block() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Message("Request".into()), &formatter);
    transcript.append(Msg::Tool("- read `src/main.rs`".into()), &formatter);
    transcript.append(
        Msg::Tool("- apply patch: src/main.rs\n\n```diff\n-old\n+new\n```".into()),
        &formatter,
    );
    transcript.append(Msg::Message("Done".into()), &formatter);

    assert_eq!(
        transcript.committed_lines(),
        &[
            "Request",
            "",
            "╭─ Tool calls",
            "│ read `src/main.rs`",
            "│ apply patch: src/main.rs",
            "╰─",
            "",
            "Done",
        ]
    );
}

#[test]
fn expanded_tools_preserve_individual_details_and_spacing() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::new(ToolDisplay::Expanded);

    transcript.append(Msg::Tool("- read `src/main.rs`".into()), &formatter);
    transcript.append(Msg::Empty, &formatter);
    transcript.append(
        Msg::Tool("- apply patch: src/main.rs\n\n```diff\n-old\n+new\n```".into()),
        &formatter,
    );

    assert_eq!(
        transcript.committed_lines(),
        &[
            "- read `src/main.rs`",
            "",
            "- apply patch: src/main.rs",
            "",
            "```diff",
            "-old",
            "+new",
            "```",
        ]
    );
}

#[test]
fn empty_streams_and_replay_spacing_do_not_split_tool_blocks() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Tool("- first".into()), &formatter);
    transcript.append(Msg::Empty, &formatter);
    transcript.start_stream(true, &formatter);
    transcript.push_stream_chunk("");
    transcript.finish_stream(true, &formatter);
    transcript.append(Msg::Tool("- second".into()), &formatter);

    assert_eq!(
        transcript.committed_lines(),
        &["╭─ Tool calls", "│ first", "│ second"]
    );
    assert_eq!(transcript.active_lines(&formatter), None);
}

#[test]
fn streamed_text_separates_tool_blocks_when_content_arrives() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Tool("- first".into()), &formatter);
    transcript.start_stream(true, &formatter);
    transcript.push_stream_chunk("Working");
    assert_eq!(
        transcript.committed_lines(),
        &["╭─ Tool calls", "│ first", "╰─", ""]
    );
    assert_eq!(
        transcript.active_lines(&formatter),
        Some(vec!["Working".into()])
    );
    transcript.finish_stream(true, &formatter);
    transcript.append(Msg::Tool("- second".into()), &formatter);

    assert_eq!(
        transcript.committed_lines(),
        &[
            "╭─ Tool calls",
            "│ first",
            "╰─",
            "",
            "Working",
            "",
            "╭─ Tool calls",
            "│ second",
        ]
    );
}

#[test]
fn scrollback_preserves_one_block_across_incremental_tool_calls() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Tool("- first".into()), &formatter);
    let mut history = transcript.take_scrollback_overflow(0, &formatter);
    assert!(history.is_empty());
    transcript.append(Msg::Tool("- second".into()), &formatter);
    history.extend(transcript.take_scrollback_overflow(0, &formatter));
    assert!(history.is_empty());
    transcript.append(Msg::Message("Done".into()), &formatter);
    history.extend(transcript.take_scrollback_overflow(0, &formatter));

    assert_eq!(
        history,
        ["╭─ Tool calls", "│ first", "│ second", "╰─", "", "Done"]
    );
}

#[test]
fn grouped_summaries_wrap_with_a_shared_border_and_ignore_empty_calls() {
    let formatter = MessageFormatter::new(20);
    let mut transcript = MessageTranscript::default();

    transcript.append(Msg::Tool(" \n\t".into()), &formatter);
    assert!(transcript.committed_lines().is_empty());
    transcript.append(
        Msg::Tool("- read\t`src/例子/a_very_long_file_name.rs`\nDetailed output".into()),
        &formatter,
    );

    let lines = transcript.committed_lines();
    assert_eq!(lines[0], "╭─ Tool calls");
    assert!(lines.len() > 2);
    assert!(lines[1..].iter().all(|line| line.starts_with("│ ")));
    assert!(
        lines
            .iter()
            .all(|line| textwrap::core::display_width(line) <= 20)
    );
    assert!(!lines.join("\n").contains("Detailed output"));
    assert!(!lines.join("\n").contains('\t'));
}

#[test]
fn clearing_resets_the_block_without_changing_the_display_mode() {
    let formatter = formatter();

    for mode in [ToolDisplay::Grouped, ToolDisplay::Expanded] {
        let mut transcript = MessageTranscript::new(mode);
        transcript.append(Msg::Tool("- old".into()), &formatter);
        transcript.clear();
        transcript.append(Msg::Tool("- new".into()), &formatter);

        let expected = match mode {
            ToolDisplay::Grouped => vec!["╭─ Tool calls", "│ new"],
            ToolDisplay::Expanded => vec!["- new"],
        };
        assert_eq!(transcript.committed_lines(), expected);
        assert!(
            !transcript
                .expanded_tool_lines(&formatter)
                .join("\n")
                .contains("old")
        );
    }
}

#[test]
fn grouped_blocks_keep_only_the_last_five_calls_and_retain_history() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();

    for call in 1..=8 {
        transcript.append(Msg::Tool(format!("- call {call}\nDetails")), &formatter);
        transcript.append(Msg::Empty, &formatter);
        assert!(
            transcript
                .take_scrollback_overflow(0, &formatter)
                .is_empty()
        );
    }
    transcript.append(Msg::Tool(" \n- \n\t".into()), &formatter);
    assert_eq!(
        transcript.committed_lines(),
        [
            "╭─ Tool calls (last 5 of 8) · Ctrl+o expand",
            "│ call 4",
            "│ call 5",
            "│ call 6",
            "│ call 7",
            "│ call 8",
        ]
    );

    transcript.append(Msg::Message("Done".into()), &formatter);
    let scrollback = transcript.take_scrollback_overflow(0, &formatter);
    assert!(!scrollback.iter().any(|line| line == "│ call 1"));
    assert!(scrollback.iter().any(|line| line == "│ call 8"));
    assert!(transcript.committed_lines().is_empty());
    let expanded = transcript.expanded_tool_lines(&formatter);
    for call in 1..=8 {
        assert!(expanded.contains(&format!("│ call {call}")));
    }
    assert!(!expanded.join("\n").contains("Details"));
}

#[test]
fn five_calls_need_no_truncation_and_each_block_has_its_own_limit() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();
    for call in 1..=5 {
        transcript.append(Msg::Tool(format!("- first {call}")), &formatter);
    }
    assert_eq!(transcript.committed_lines().len(), 6);
    assert_eq!(transcript.committed_lines()[0], "╭─ Tool calls");
    transcript.append(Msg::Message("Between blocks".into()), &formatter);
    for call in 1..=6 {
        transcript.append(Msg::Tool(format!("- second {call}")), &formatter);
    }
    assert!(transcript.committed_lines().contains(&"│ first 1".into()));
    assert!(!transcript.committed_lines().contains(&"│ second 1".into()));
    assert_eq!(transcript.expanded_tool_lines(&formatter).len(), 17);
}

#[test]
fn preceding_scrollback_does_not_move_the_active_tool_block_start() {
    let formatter = formatter();
    let mut transcript = MessageTranscript::default();
    transcript.append(Msg::Message("Request".into()), &formatter);
    transcript.append(Msg::Tool("- first".into()), &formatter);
    assert_eq!(
        transcript.take_scrollback_overflow(0, &formatter),
        ["Request", ""]
    );
    for call in 1..=5 {
        transcript.append(Msg::Tool(format!("- later {call}")), &formatter);
    }
    assert_eq!(transcript.committed_lines().len(), 6);
    assert!(!transcript.committed_lines().contains(&"│ first".into()));
    assert!(transcript.committed_lines().contains(&"│ later 5".into()));
}

#[test]
fn recent_limit_counts_calls_instead_of_wrapped_lines() {
    let formatter = MessageFormatter::new(20);
    let mut transcript = MessageTranscript::default();
    transcript.append(Msg::Tool("- oldest".into()), &formatter);
    for call in 1..=5 {
        transcript.append(
            Msg::Tool(format!("- call {call} with a long wrapped summary")),
            &formatter,
        );
    }
    let lines = transcript.committed_lines();
    assert!(lines.len() > 6);
    assert!(!lines.iter().any(|line| line.contains("oldest")));
    for call in 1..=5 {
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("call {call}")))
        );
    }
    assert!(
        lines
            .iter()
            .all(|line| textwrap::core::display_width(line) <= 20)
    );
}
