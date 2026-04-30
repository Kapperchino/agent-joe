
pub fn wrap_input_text(
    text: &str,
    wrap_width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
) -> Vec<String> {
    let wrap_width = wrap_width.max(1);
    let initial_indent_width = textwrap::core::display_width(initial_indent);
    let subsequent_indent_width = textwrap::core::display_width(subsequent_indent);

    let mut lines = Vec::new();
    let mut current_line = initial_indent.to_string();
    let mut current_width = initial_indent_width;

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current_line);
            current_line = subsequent_indent.to_string();
            current_width = subsequent_indent_width;
            continue;
        }

        let ch_width = textwrap::core::display_width(ch.encode_utf8(&mut [0; 4]));
        if current_width + ch_width > wrap_width {
            lines.push(current_line);
            current_line = subsequent_indent.to_string();
            current_width = subsequent_indent_width;
        }

        current_line.push(ch);
        current_width += ch_width;
    }

    lines.push(current_line);
    lines
}
