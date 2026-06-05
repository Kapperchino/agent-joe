use crate::utils::draw_table::DrawTable;
use markdown::ParseOptions;
use markdown::mdast::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StreamSplit {
    pub(super) prefix: String,
    pub(super) suffix: String,
}

pub(super) fn split_stream_to_fit(
    message: &str,
    max_live_lines: usize,
    wrap_width: usize,
) -> Option<StreamSplit> {
    if max_live_lines == 0 {
        return None;
    }

    let lines = message.split('\n').collect::<Vec<_>>();
    (1..lines.len())
        .filter_map(|split_line| split_at_line(&lines, split_line, wrap_width))
        .find(|split| {
            !split.suffix.is_empty()
                && DrawTable::wrap_markdown_tables(&split.suffix, wrap_width).len()
                    <= max_live_lines
        })
}

pub(super) fn flush_count_preserving_tables(lines: &[String], requested_flush: usize) -> usize {
    if requested_flush == 0 || requested_flush >= lines.len() {
        return requested_flush;
    }

    let markdown = lines.join("\n");
    let tree = match markdown::to_mdast(&markdown, &markdown_parse_options()) {
        Ok(tree) => tree,
        Err(_) => return requested_flush,
    };

    table_boundary(&tree, requested_flush)
        .unwrap_or(requested_flush)
        .min(lines.len())
}

fn split_at_line(lines: &[&str], split_line: usize, wrap_width: usize) -> Option<StreamSplit> {
    if !(1..lines.len()).contains(&split_line) {
        return None;
    }

    DrawTable::table_block_spanning_split(lines, split_line)
        .map(|(table_start, table_end)| {
            split_table_at_line(lines, split_line, table_start, table_end, wrap_width)
        })
        .unwrap_or_else(|| Some(split_text_at_line(lines, split_line)))
}

fn split_text_at_line(lines: &[&str], split_line: usize) -> StreamSplit {
    StreamSplit {
        prefix: lines[..split_line].join("\n"),
        suffix: lines[split_line..].join("\n"),
    }
}

fn split_table_at_line(
    lines: &[&str],
    split_line: usize,
    table_start: usize,
    table_end: usize,
    wrap_width: usize,
) -> Option<StreamSplit> {
    if split_line <= table_start + 2 {
        return None;
    }

    let width_hint = DrawTable::table_width_hint(lines, table_start, table_end, wrap_width);
    let mut prefix_lines = lines[..split_line]
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();

    if let Some(header) = prefix_lines.get_mut(table_start) {
        *header = DrawTable::mark_table_header_with_width_hint(header, width_hint.as_deref());
    }

    let mut suffix_lines = Vec::with_capacity(lines.len().saturating_sub(split_line) + 2);
    suffix_lines.push(DrawTable::mark_table_header_as_continuation(
        lines[table_start],
        width_hint.as_deref(),
    ));
    suffix_lines.push(lines[table_start + 1].to_string());
    suffix_lines.extend(lines[split_line..].iter().map(|line| (*line).to_string()));

    Some(StreamSplit {
        prefix: prefix_lines.join("\n"),
        suffix: suffix_lines.join("\n"),
    })
}

fn table_boundary(node: &Node, requested_flush: usize) -> Option<usize> {
    let node_boundary = match node {
        Node::Table(table) => table_end_line(table).and_then(|end_line| {
            let start_line = table
                .position
                .as_ref()
                .map(|position| position.start.line)
                .unwrap_or(0);

            if start_line <= requested_flush && requested_flush < end_line {
                Some(end_line)
            } else {
                None
            }
        }),
        _ => None,
    };

    let child_boundary = node.children().and_then(|children| {
        children
            .iter()
            .filter_map(|child| table_boundary(child, requested_flush))
            .max()
    });

    node_boundary.into_iter().chain(child_boundary).max()
}

fn table_end_line(table: &markdown::mdast::Table) -> Option<usize> {
    table
        .children
        .iter()
        .filter_map(|child| match child {
            Node::TableRow(row) => row.position.as_ref().map(position_end_line),
            _ => None,
        })
        .max()
        .or_else(|| table.position.as_ref().map(position_end_line))
}

fn position_end_line(position: &markdown::unist::Position) -> usize {
    if position.end.column == 1 {
        position.end.line.saturating_sub(1)
    } else {
        position.end.line
    }
}

fn markdown_parse_options() -> ParseOptions {
    let mut options = ParseOptions::default();
    options.constructs.gfm_table = true;
    options
}

#[cfg(test)]
mod tests {
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
}
