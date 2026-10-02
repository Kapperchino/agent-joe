use super::*;
use crate::widgets::message_box::format::MessageFormatter;
use crate::widgets::message_box::message_box::{MessageBox, MessageBoxState, Msg, ToolDisplay};
use crate::widgets::message_box::scrollback::ScrollbackRenderer;
use crate::widgets::message_box::transcript::MessageTranscript;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};

fn text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[test]
fn grouped_tools_use_palette_badges_rails_and_argument_highlights() {
    let formatter = MessageFormatter::new(80);
    let mut transcript = MessageTranscript::default();
    transcript.append(
        Msg::Tool("- read `src/main.rs` (lines 1-8)".into()),
        &formatter,
    );
    let renderer = TranscriptRenderer::new(80);
    let lines = renderer.render_lines(
        transcript.committed_rows(),
        &mut TranscriptRenderState::default(),
    );

    let title = &lines[0].spans[1];
    assert_eq!(title.content, "Tool calls");
    assert_eq!(title.style.fg, Some(theme::BACKGROUND));
    assert_eq!(title.style.bg, Some(theme::ACCENT));
    assert!(title.style.add_modifier.contains(Modifier::BOLD));
    let entry = &lines[1];
    assert_eq!(text(entry), "│ read src/main.rs (lines 1-8)");
    assert_eq!(entry.spans[0].style.fg, Some(theme::BORDER));
    assert_eq!(entry.spans[1].style.fg, Some(theme::ACCENT));
    assert!(entry.spans[1].style.add_modifier.contains(Modifier::BOLD));
    let path = entry
        .spans
        .iter()
        .find(|span| span.content == "src/main.rs")
        .unwrap();
    assert_eq!(path.style.fg, Some(theme::AMBER));
    assert_eq!(path.style.bg, Some(theme::SELECTION));
    assert!(
        lines
            .iter()
            .all(|line| line.style.bg == Some(theme::SURFACE))
    );
}

#[test]
fn live_scrollback_and_history_share_identical_tool_styling() {
    let formatter = MessageFormatter::new(80);
    let mut transcript = MessageTranscript::default();
    transcript.append(Msg::Tool("- grep `ToolDisplay`".into()), &formatter);
    transcript.append(Msg::Tool("- git status".into()), &formatter);
    transcript.append(Msg::Message("Done".into()), &formatter);
    let mut renderer = ScrollbackRenderer::new();

    let live = renderer.render_live_lines(transcript.committed_rows(), None, None);
    let history = renderer.render_history_lines(transcript.committed_rows());
    let rows = transcript.take_scrollback_rows(0, &formatter);
    let flushed = renderer.render_flushed_lines(&rows);

    assert_eq!(live, history);
    assert_eq!(live, flushed);
    assert_eq!(live.last().unwrap().style.bg, None);
}

#[test]
fn expanded_tools_keep_diff_colors_on_the_shared_surface() {
    let formatter = MessageFormatter::new(80);
    let rows = formatter.format_msg(Msg::Tool(
        "knowledge read `src/main.rs`\n\n```diff\n-old\n+new\n```".into(),
    ));
    let renderer = TranscriptRenderer::new(80);
    let lines = renderer.render_lines(&rows, &mut TranscriptRenderState::default());

    assert_eq!(text(&lines[0]), "│ knowledge read src/main.rs");
    for (content, color) in [("-old", theme::RED), ("+new", theme::GREEN)] {
        let line = lines
            .iter()
            .find(|line| text(line).contains(content))
            .unwrap();
        assert_eq!(line.spans[0].content, "│ ");
        assert_eq!(line.spans[0].style.fg, Some(theme::BORDER));
        assert_eq!(line.spans[1].style.fg, Some(color));
        assert_eq!(line.style.bg, Some(theme::SURFACE));
    }
}

#[test]
fn ordinary_markdown_and_code_are_not_treated_as_tools() {
    let source = vec![
        "╭─ Tool calls".into(),
        "│ read `src/main.rs`".into(),
        "".into(),
        "- an ordinary list".into(),
        "```text".into(),
        "│ read `not a tool`".into(),
        "```".into(),
    ];
    let rows = source
        .iter()
        .cloned()
        .map(TranscriptLine::message)
        .collect::<Vec<_>>();
    let renderer = TranscriptRenderer::new(80);
    let rendered = renderer.render_lines(&rows, &mut TranscriptRenderState::default());

    assert_eq!(rendered, DrawLine::new().render_lines(&source));
    assert!(
        rendered
            .iter()
            .all(|line| line.style.bg != Some(theme::SURFACE))
    );
}

#[test]
fn wrapped_arguments_retain_highlights_across_scrollback_batches() {
    let rows = vec![
        TranscriptLine::new(LineKind::ToolEntry, "│ read `src/"),
        TranscriptLine::new(LineKind::ToolContinuation, "│ main.rs` (lines 1-8)"),
    ];
    let renderer = TranscriptRenderer::new(80);
    let whole = renderer.render_lines(&rows, &mut TranscriptRenderState::default());
    let mut state = TranscriptRenderState::default();
    let mut split = renderer.render_lines(&rows[..1], &mut state);
    split.extend(renderer.render_lines(&rows[1..], &mut state));

    assert_eq!(whole, split);
    let path = split[1]
        .spans
        .iter()
        .find(|span| span.content == "main.rs")
        .unwrap();
    assert_eq!(path.style.fg, Some(theme::AMBER));
    assert_eq!(path.style.bg, Some(theme::SELECTION));
    let suffix = split[1]
        .spans
        .iter()
        .find(|span| span.content.contains("lines 1-8"))
        .unwrap();
    assert_eq!(suffix.style.fg, Some(theme::MUTED));
}

#[test]
fn tool_detail_fences_continue_across_batches_without_leaking_into_chat() {
    let renderer = TranscriptRenderer::new(80);
    let mut state = TranscriptRenderState::default();
    renderer.render_lines(
        &[
            TranscriptLine::new(LineKind::ToolDetail, "```diff"),
            TranscriptLine::new(LineKind::ToolDetail, "-old"),
        ],
        &mut state,
    );
    let continuation = renderer.render_lines(
        &[
            TranscriptLine::new(LineKind::ToolDetail, "+new"),
            TranscriptLine::message("# Done"),
        ],
        &mut state,
    );

    assert_eq!(continuation[0].spans[1].style.fg, Some(theme::GREEN));
    assert_eq!(text(&continuation[1]), "Done");
    assert_eq!(continuation[1].style.bg, None);
    assert_eq!(continuation[1].spans[0].style.fg, Some(theme::ACCENT));
}

#[test]
fn narrow_tool_summaries_preserve_width_and_unicode_content() {
    for width in [4, 12, 20] {
        let renderer = TranscriptRenderer::new(width);
        let formatter = MessageFormatter::new(width);
        let mut transcript = MessageTranscript::default();
        transcript.append(
            Msg::Tool("- read `src/例子/main.rs` with a long summary".into()),
            &formatter,
        );
        transcript.append(Msg::Message("Done".into()), &formatter);
        let lines = renderer.render_lines(
            transcript.committed_rows(),
            &mut TranscriptRenderState::default(),
        );

        assert!(
            lines.iter().all(|line| line.width() <= width),
            "width {width}: {lines:?}"
        );
        let content = lines.iter().map(text).collect::<String>();
        assert!(content.contains('例'));
        assert!(content.contains('子'));
    }
}

#[test]
fn widget_and_agent_history_render_the_same_styled_cells() {
    let area = Rect::new(0, 0, 60, 10);
    let mut state = MessageBoxState::with_tool_display(ToolDisplay::Grouped);
    state.update_width_height(area.width, area.height);
    state.append(Msg::Tool("- read `src/main.rs`".into()));
    let mut live = Buffer::empty(area);
    MessageBox {}.render(area, &mut live, &mut state);
    let mut agent = Buffer::empty(area);
    state.render_history(area, &mut agent, 0);

    assert_eq!(live, agent);
    assert_eq!(live[(3, 0)].bg, theme::ACCENT);
    assert_eq!(live[(0, 1)].fg, theme::BORDER);
    assert_eq!(live[(2, 1)].fg, theme::ACCENT);
    assert_eq!(live[(8, 1)].fg, theme::AMBER);
    assert_eq!(live[(50, 1)].bg, theme::SURFACE);

    state.handle_tool_history_key(&KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let mut expanded = Buffer::empty(area);
    MessageBox {}.render(area, &mut expanded, &mut state);
    assert_eq!(expanded[(1, 0)].bg, theme::ACCENT);
    assert_eq!(expanded[(2, 2)].fg, theme::ACCENT);
    assert_eq!(expanded[(8, 2)].fg, theme::AMBER);
}
