use super::*;

#[test]
fn parses_rows_without_leading_or_trailing_pipes() {
    assert_eq!(
        DrawTable::parse_table_row("left | right"),
        Some(vec!["left".to_string(), "right".to_string()])
    );
}

#[test]
fn does_not_treat_mismatched_header_and_delimiter_as_a_table() {
    let markdown = "left | right\n--- | --- | ---\none | two";

    assert_eq!(
        DrawTable::wrap_markdown_tables(markdown, 80),
        markdown.lines().map(str::to_string).collect::<Vec<_>>()
    );
}

#[test]
fn does_not_wrap_table_like_content_inside_tilde_fences() {
    let markdown = "~~~text\nvery long | table-like content\n--- | ---\n~~~";

    assert_eq!(
        DrawTable::wrap_markdown_tables(markdown, 10),
        markdown.lines().map(str::to_string).collect::<Vec<_>>()
    );
}

#[test]
fn clamps_zero_wrap_width() {
    let wrapped = DrawTable::wrap_markdown_tables("abc", 0);

    assert!(!wrapped.is_empty());
    assert!(wrapped.iter().all(|line| display_width(line) <= 1));
}
