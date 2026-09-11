use super::*;

#[test]
fn flush_extends_to_end_of_table() {
    let lines = vec![
        "| Col |".to_string(),
        "| --- |".to_string(),
        "| one |".to_string(),
        "| two |".to_string(),
        String::new(),
        "after".to_string(),
    ];

    assert_eq!(flush_count_preserving_tables(&lines, 2), 4);
}

#[test]
fn split_stream_inside_table_repeats_table_context_in_suffix() {
    let message = ["before", "| Col |", "| --- |", "| one |", "| two |"].join("\n");

    let split = split_stream_to_fit(&message, 3, 40).expect("stream should split");

    assert_eq!(split.prefix.lines().next(), Some("before"));
    assert!(split.suffix.contains("<!--__table_block_continue__-->"));
    assert!(split.suffix.contains("| --- |"));
    assert!(split.suffix.contains("| two |"));
}
