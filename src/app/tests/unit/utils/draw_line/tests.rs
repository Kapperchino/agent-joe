use super::*;
use crate::utils::draw_table::DrawTable;

fn line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn renders_diff_fence_with_info_string_and_add_remove_colors() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "```diff changes.patch".to_string(),
        "diff --git a/file b/file".to_string(),
        "@@".to_string(),
        "-old".to_string(),
        "+new".to_string(),
        " context".to_string(),
        "```".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);

    assert_eq!(rendered[0].spans[0].style.fg, Some(theme::MUTED));
    assert_eq!(rendered[1].spans[0].style.fg, Some(theme::ACCENT));
    assert_eq!(rendered[2].spans[0].style.fg, Some(theme::RED));
    assert_eq!(rendered[3].spans[0].style.fg, Some(theme::GREEN));
    assert_eq!(rendered[4].spans[0].style.fg, None);
}

#[test]
fn markdown_uses_warm_accents_and_retains_formatting_cues() {
    let draw_line = DrawLine::new();
    let rendered = draw_line.render_lines(&[
        "# First".into(),
        "## Second".into(),
        "### Third".into(),
        "`code` and [link](https://example.com)".into(),
        "- item".into(),
    ]);
    let spans = rendered
        .iter()
        .flat_map(|line| &line.spans)
        .collect::<Vec<_>>();
    for (text, color, modifier) in [
        ("First", theme::ACCENT, Modifier::BOLD),
        ("Second", theme::AMBER, Modifier::BOLD),
        ("Third", theme::TEXT, Modifier::BOLD),
        ("code", theme::AMBER, Modifier::BOLD),
        ("link", theme::ACCENT, Modifier::UNDERLINED),
        ("• ", theme::ACCENT, Modifier::BOLD),
    ] {
        let span = spans.iter().find(|span| span.content == text).unwrap();
        assert_eq!(span.style.fg, Some(color), "{text}");
        assert!(span.style.add_modifier.contains(modifier), "{text}");
    }
    let url = spans
        .iter()
        .find(|span| span.content.contains("https://example.com"))
        .unwrap();
    assert_eq!(url.style.fg, Some(theme::MUTED));
}

#[test]
fn extracts_language_from_fence_info_string() {
    assert_eq!(
        CodeFence::language("rust src/lib.rs").as_deref(),
        Some("rust")
    );
    assert_eq!(CodeFence::language("rust,ignore").as_deref(), Some("rust"));
    assert_eq!(CodeFence::language("{.rust}").as_deref(), Some("rust"));
    assert_eq!(
        CodeFence::language("language-rust").as_deref(),
        Some("rust")
    );
}

#[test]
fn indented_markdown_code_does_not_force_yellow_text() {
    let draw_line = DrawLine::new();
    let lines = vec!["    parser fallback".to_string()];

    let rendered = draw_line.render_lines(&lines);

    assert_eq!(rendered[0].spans[0].style.fg, None);
    assert_eq!(line_text(&rendered[0]), "    parser fallback");
}

#[test]
fn preserves_root_indentation_when_rendering_partial_markdown() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "  wrapped list continuation".to_string(),
        "   - orphaned nested item".to_string(),
        "- next root item".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "  wrapped list continuation".to_string(),
            "   • orphaned nested item".to_string(),
            "• next root item".to_string(),
        ]
    );
}

#[test]
fn trims_padding_between_heading_and_fenced_code_content() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "### take_scrollback_overflow".to_string(),
        "```rust".to_string(),
        String::new(),
        String::new(),
        "pub(super) fn take_scrollback_overflow(".to_string(),
        "```".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "take_scrollback_overflow".to_string(),
            "pub(super) fn take_scrollback_overflow(".to_string(),
        ]
    );
}

#[test]
fn preserves_blank_lines_inside_fenced_code_content() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "```rust".to_string(),
        "fn one() {}".to_string(),
        String::new(),
        "fn two() {}".to_string(),
        "```".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "fn one() {}".to_string(),
            String::new(),
            "fn two() {}".to_string(),
        ]
    );
}

#[test]
fn keeps_code_fence_state_across_render_batches() {
    let draw_line = DrawLine::new();
    let mut state = RenderState::default();

    let first =
        draw_line.render_lines_with_state(&["```diff".to_string(), "-old".to_string()], &mut state);
    let second = draw_line.render_lines_with_state(
        &["+new".to_string(), "```".to_string(), "after".to_string()],
        &mut state,
    );

    assert_eq!(first[0].spans[0].style.fg, Some(theme::RED));
    assert_eq!(second[0].spans[0].style.fg, Some(theme::GREEN));
    assert_eq!(line_text(&second[1]), "after");
    assert!(state.fence.is_none());
}

#[test]
fn preserves_blank_code_prefix_when_fence_started_in_previous_batch() {
    let draw_line = DrawLine::new();
    let mut state = RenderState::default();

    draw_line.render_lines_with_state(&["```rust".to_string()], &mut state);
    let rendered = draw_line.render_lines_with_state(
        &[String::new(), "fn main() {}".to_string(), "```".to_string()],
        &mut state,
    );
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(text, vec![String::new(), "fn main() {}".to_string()]);
    assert!(state.fence.is_none());
}

#[test]
fn language_fence_inside_code_content_does_not_close_block() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "```md".to_string(),
        "```rust".to_string(),
        "fn main() {}".to_string(),
        "```".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec!["```rust".to_string(), "fn main() {}".to_string()]
    );
}

#[test]
fn renders_tilde_fences_and_requires_matching_closing_marker() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "~~~rust".to_string(),
        "fn main() {}".to_string(),
        "```".to_string(),
        "~~~".to_string(),
        "after".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "fn main() {}".to_string(),
            "```".to_string(),
            "after".to_string(),
        ]
    );
}

#[test]
fn recovers_markdown_list_after_elided_unclosed_code_fence() {
    let draw_line = DrawLine::new();
    let mut state = RenderState::default();
    let lines = vec![
        "```rust".to_string(),
        "    ...".to_string(),
        String::new(),
        String::new(),
        "- **Render diffs specially**".to_string(),
        "  - Detects diff-like languages.".to_string(),
    ];

    let rendered = draw_line.render_lines_with_state(&lines, &mut state);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "    ...".to_string(),
            String::new(),
            String::new(),
            "• Render diffs specially".to_string(),
            "  • Detects diff-like languages.".to_string(),
        ]
    );
    assert!(state.fence.is_none());
}

#[test]
fn diff_fences_do_not_recover_on_removed_markdown_like_lines() {
    let draw_line = DrawLine::new();
    let mut state = RenderState::default();
    let lines = vec![
        "```diff".to_string(),
        " context".to_string(),
        String::new(),
        "- **removed heading**".to_string(),
    ];

    let rendered = draw_line.render_lines_with_state(&lines, &mut state);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            " context".to_string(),
            String::new(),
            "- **removed heading**".to_string(),
        ]
    );
    assert_eq!(rendered[2].spans[0].style.fg, Some(theme::RED));
    assert!(state.fence.is_some());
}

#[test]
fn renders_wrapped_markdown_lists_without_raw_markers_or_flush_left_continuations() {
    let draw_line = DrawLine::new();
    let wrapped = DrawTable::wrap_markdown_tables(
        "splits it into:\n- prefix: committed to history\n- suffix: remains active\n- Uses table_flow::split_stream_to_fit, preserving table context when splitting markdown tables.",
        68,
    );

    let rendered = draw_line.render_lines(&wrapped);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(text[0], "splits it into:");
    assert_eq!(text[1], "• prefix: committed to history");
    assert_eq!(text[2], "• suffix: remains active");
    assert!(text.iter().all(|line| !line.starts_with("- ")));
    assert!(
        text.iter()
            .any(|line| line.starts_with("  when splitting markdown tables."))
    );
}

#[test]
fn renders_loose_nested_list_continuations_as_nested_bullets() {
    let draw_line = DrawLine::new();
    let lines = vec![
        "- DrawLine".to_string(),
        "- Holds:".to_string(),
        " - `SyntaxSet` from `syntect` for syntax lookup.".to_string(),
        " - `ThemeSet` from `syntect` for code highlighting.".to_string(),
        "- Created with `DrawLine::new()`.".to_string(),
    ];

    let rendered = draw_line.render_lines(&lines);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        text,
        vec![
            "• DrawLine".to_string(),
            "• Holds:".to_string(),
            "  • SyntaxSet from syntect for syntax lookup.".to_string(),
            "  • ThemeSet from syntect for code highlighting.".to_string(),
            "• Created with DrawLine::new().".to_string(),
        ]
    );
}

#[test]
fn wrapping_preserves_nested_list_initial_indent() {
    let wrapped = DrawTable::wrap_markdown_tables(
        "  - `SyntaxSet` from `syntect` for syntax lookup and another long phrase",
        36,
    );

    assert!(wrapped[0].starts_with("  - "));
    assert!(wrapped[1].starts_with("    "));
}

#[test]
fn wraps_long_table_headers_to_the_viewport_width() {
    let draw_line = DrawLine::new();
    let wrapped = DrawTable::wrap_markdown_tables(
        "| This table header is much too long |\n| --- |\n| value |",
        12,
    );
    let rendered = draw_line.render_lines(&wrapped);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();
    let separator_index = text
        .iter()
        .position(|line| !line.is_empty() && line.chars().all(|ch| ch == '─'))
        .expect("table separator should be rendered");

    assert!(separator_index > 1, "header should wrap: {text:?}");
    assert!(
        text.iter().all(|line| display_width(line) <= 12),
        "rendered table overflowed: {text:?}"
    );
}

#[test]
fn expands_tabs_at_four_column_stops() {
    assert_eq!(DrawLine::expand_tabs("\talpha"), "    alpha");
    assert_eq!(DrawLine::expand_tabs("ab\talpha"), "ab  alpha");
    assert_eq!(DrawLine::expand_tabs("abc\talpha"), "abc alpha");
}

#[test]
fn tab_indented_nested_lists_wrap_and_render_consistently() {
    let draw_line = DrawLine::new();
    let wrapped = DrawTable::wrap_markdown_tables(
        "- parent\n\t- child content that wraps onto another line",
        30,
    );

    assert!(wrapped.iter().all(|line| !line.contains('\t')));
    assert!(wrapped[1].starts_with("    - "));
    assert!(wrapped[2].starts_with("      "));

    let rendered = draw_line.render_lines(&wrapped);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(text[0], "• parent");
    assert!(text[1].starts_with("  • child content"));
    assert!(text[2].starts_with("    "));
}

#[test]
fn expands_tabs_in_fenced_code_without_wrapping_code_lines() {
    let draw_line = DrawLine::new();
    let wrapped =
        DrawTable::wrap_markdown_tables("```rust\n\tlet value = 1;\nvalue\t+= 1;\n```", 12);
    let rendered = draw_line.render_lines(&wrapped);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(text, vec!["    let value = 1;", "value   += 1;"]);
}
