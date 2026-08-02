use crate::utils::draw_line::DrawLine;
use textwrap::core::display_width;

pub struct DrawTable {}

const TABLE_CONTINUATION_MARKER: &str = "<!--__table_continue__-->";
const TABLE_BLOCK_CONTINUATION_MARKER: &str = "<!--__table_block_continue__-->";
const TABLE_WIDTH_MARKER_PREFIX: &str = "<!--__table_widths__:";
const HTML_COMMENT_SUFFIX: &str = "-->";

#[derive(Clone, Copy)]
enum TableAlignment {
    Left,
    Right,
    Center,
    None,
}

#[derive(Clone, Copy)]
struct CodeFence {
    marker: char,
    len: usize,
}

impl DrawTable {
    pub(crate) fn mark_table_header_as_continuation(
        line: &str,
        width_hint: Option<&[usize]>,
    ) -> String {
        let Some(mut cells) = Self::parse_table_row(line) else {
            return line.to_string();
        };

        if let Some(first_cell) = cells.first_mut() {
            if !first_cell.starts_with(TABLE_BLOCK_CONTINUATION_MARKER) {
                first_cell.insert_str(0, TABLE_BLOCK_CONTINUATION_MARKER);
            }
            if Self::table_width_hint_from_cell(first_cell).is_none() {
                if let Some(widths) = width_hint {
                    first_cell.insert_str(
                        TABLE_BLOCK_CONTINUATION_MARKER.len(),
                        &Self::format_table_width_marker(widths),
                    );
                }
            }
        }

        Self::format_table_row(&cells)
    }

    pub fn table_block_spanning_split(lines: &[&str], split_line: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        while start + 1 < lines.len() {
            let Some(end) = DrawTable::table_block_end(lines, start) else {
                start += 1;
                continue;
            };

            if start < split_line && split_line < end {
                return Some((start, end));
            }

            start = end.max(start + 1);
        }

        None
    }

    pub(crate) fn table_width_hint(
        lines: &[&str],
        start: usize,
        end: usize,
        wrap_width: usize,
    ) -> Option<Vec<usize>> {
        let alignments = Self::parse_table_alignments(*lines.get(start + 1)?)?;
        let column_count = alignments.len();
        let header = Self::parse_table_row(*lines.get(start)?)?;
        if header.len() != column_count {
            return None;
        }

        if let Some(widths) = Self::table_width_hint_from_cells(&header) {
            return Some(Self::normalize_table_width_hint(
                widths,
                column_count,
                wrap_width,
            ));
        }

        let body_rows = lines[start + 2..end]
            .iter()
            .map(|line| Self::parse_table_row(line))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(|row| Self::normalize_table_cells(row, column_count))
            .collect::<Vec<_>>();

        Some(Self::table_column_widths(&header, &body_rows, wrap_width))
    }

    fn wrap_table_block(lines: &[&str], wrap_width: usize) -> Option<(usize, Vec<String>)> {
        if lines.len() < 2 {
            return None;
        }

        let alignments = Self::parse_table_alignments(lines[1])?;
        let column_count = alignments.len();
        let header = Self::parse_table_row(lines[0])?;
        if header.len() != column_count {
            return None;
        }
        let width_hint = Self::table_width_hint_from_cells(&header)
            .map(|widths| Self::normalize_table_width_hint(widths, column_count, wrap_width));

        let mut body_rows = Vec::new();
        let mut consumed = 2;
        while consumed < lines.len() {
            let line = lines[consumed];
            if line.trim().is_empty() {
                break;
            }

            let Some(row) = Self::parse_table_row(line) else {
                break;
            };
            body_rows.push(Self::normalize_table_cells(row, column_count));
            consumed += 1;
        }

        let block = Self::wrap_table_rows(
            &header,
            &alignments,
            &body_rows,
            wrap_width,
            width_hint.as_deref(),
        );
        Some((consumed, block))
    }

    fn table_column_widths(
        header: &[String],
        body_rows: &[Vec<String>],
        wrap_width: usize,
    ) -> Vec<usize> {
        let column_count = header.len();
        let separator_width = column_count.saturating_sub(1) * 3;
        let max_content_width = wrap_width.saturating_sub(separator_width).max(column_count);

        let mut widths = vec![1usize; column_count];
        for (column, cell) in header.iter().enumerate() {
            widths[column] = widths[column].max(display_width(&Self::strip_table_markers(cell)));
        }
        for row in body_rows {
            for (column, cell) in row.iter().enumerate() {
                widths[column] =
                    widths[column].max(display_width(&Self::strip_table_markers(cell)));
            }
        }

        while widths.iter().sum::<usize>() > max_content_width {
            let Some((column, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, width)| **width > 1)
                .max_by_key(|(_, width)| **width)
            else {
                break;
            };
            widths[column] -= 1;
        }

        widths
    }

    fn normalize_table_width_hint(
        mut widths: Vec<usize>,
        column_count: usize,
        wrap_width: usize,
    ) -> Vec<usize> {
        widths.truncate(column_count);
        while widths.len() < column_count {
            widths.push(1);
        }
        for width in &mut widths {
            *width = (*width).max(1);
        }

        let separator_width = column_count.saturating_sub(1) * 3;
        let max_content_width = wrap_width.saturating_sub(separator_width).max(column_count);
        while widths.iter().sum::<usize>() > max_content_width {
            let Some((column, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, width)| **width > 1)
                .max_by_key(|(_, width)| **width)
            else {
                break;
            };
            widths[column] -= 1;
        }

        widths
    }

    fn format_table_row(cells: &[String]) -> String {
        format!("| {} |", cells.join(" | "))
    }

    fn format_table_delimiter(alignments: &[TableAlignment], widths: &[usize]) -> String {
        let cells = alignments
            .iter()
            .zip(widths.iter().copied())
            .map(|(alignment, width)| {
                let width = width.max(3);
                match alignment {
                    TableAlignment::Left => {
                        format!(":{}", "-".repeat(width.saturating_sub(1)))
                    }
                    TableAlignment::Right => {
                        format!("{}:", "-".repeat(width.saturating_sub(1)))
                    }
                    TableAlignment::Center => {
                        format!(":{}:", "-".repeat(width.saturating_sub(2).max(1)))
                    }
                    TableAlignment::None => "-".repeat(width),
                }
            })
            .collect::<Vec<_>>();

        Self::format_table_row(&cells)
    }

    fn normalize_table_cells(mut cells: Vec<String>, column_count: usize) -> Vec<String> {
        cells.truncate(column_count);
        while cells.len() < column_count {
            cells.push(String::new());
        }
        cells
    }

    fn strip_table_markers(cell: &str) -> String {
        Self::strip_table_width_markers(
            &cell
                .replace(TABLE_CONTINUATION_MARKER, "")
                .replace(TABLE_BLOCK_CONTINUATION_MARKER, ""),
        )
    }

    fn strip_table_width_markers(cell: &str) -> String {
        let mut stripped = cell.to_string();

        while let Some(start) = stripped.find(TABLE_WIDTH_MARKER_PREFIX) {
            let marker_start = start + TABLE_WIDTH_MARKER_PREFIX.len();
            let Some(marker_end) = stripped[marker_start..].find(HTML_COMMENT_SUFFIX) else {
                break;
            };
            stripped.replace_range(
                start..marker_start + marker_end + HTML_COMMENT_SUFFIX.len(),
                "",
            );
        }

        stripped
    }

    fn wrap_plain_line(
        line: &str,
        wrap_width: usize,
        preserve_markdown_list_indent: bool,
    ) -> Vec<String> {
        if display_width(line) <= wrap_width {
            return vec![line.to_string()];
        }

        let mut options = textwrap::Options::new(wrap_width);
        let initial_indent = preserve_markdown_list_indent
            .then(|| DrawLine::markdown_list_initial_indent(line))
            .flatten();
        let list_indent = preserve_markdown_list_indent
            .then(|| DrawLine::markdown_list_content_indent(line))
            .flatten();
        if let Some(indent) = initial_indent.as_deref() {
            options = options.initial_indent(indent);
        }
        if let Some(indent) = list_indent.as_deref() {
            options = options.subsequent_indent(indent);
        }

        let line = initial_indent
            .as_ref()
            .map(|indent| &line[indent.len()..])
            .unwrap_or(line);

        textwrap::wrap(line, options)
            .into_iter()
            .map(|segment| segment.into_owned())
            .collect()
    }

    pub(crate) fn wrap_markdown_tables(text: &str, wrap_width: usize) -> Vec<String> {
        let wrap_width = wrap_width.max(1);
        let expanded_lines = text
            .split('\n')
            .map(DrawLine::expand_tabs)
            .collect::<Vec<_>>();
        let lines = expanded_lines
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut wrapped = Vec::new();
        let mut index = 0;
        let mut code_fence = None;

        while index < lines.len() {
            let line = lines[index];
            let was_in_code = code_fence.is_some();

            if code_fence.is_none() {
                if let Some((consumed, table_lines)) =
                    Self::wrap_table_block(&lines[index..], wrap_width)
                {
                    wrapped.extend(table_lines);
                    index += consumed;
                    continue;
                }
            }

            if let Some(fence) = code_fence {
                if Self::is_closing_code_fence(line, fence) {
                    code_fence = None;
                }
            } else {
                code_fence = Self::opening_code_fence(line);
            }

            if was_in_code || code_fence.is_some() {
                wrapped.push(line.to_string());
            } else {
                wrapped.extend(Self::wrap_plain_line(line, wrap_width, true));
            }
            index += 1;
        }

        wrapped
    }

    fn opening_code_fence(line: &str) -> Option<CodeFence> {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next()?;
        if !matches!(marker, '`' | '~') {
            return None;
        }

        let len = trimmed.chars().take_while(|&ch| ch == marker).count();
        if len < 3 {
            return None;
        }

        let info = trimmed[len..].trim();
        if marker == '`' && info.contains('`') {
            return None;
        }

        Some(CodeFence { marker, len })
    }

    fn is_closing_code_fence(line: &str, fence: CodeFence) -> bool {
        let trimmed = line.trim();
        let len = trimmed
            .chars()
            .take_while(|&ch| ch == fence.marker)
            .count();
        len >= fence.len && trimmed.len() == len
    }

    pub(crate) fn table_block_end(lines: &[&str], start: usize) -> Option<usize> {
        let alignments = Self::parse_table_alignments(*lines.get(start + 1)?)?;
        let column_count = alignments.len();
        if Self::parse_table_row(*lines.get(start)?)?.len() != column_count {
            return None;
        }

        let mut end = start + 2;
        while end < lines.len() {
            let line = lines[end];
            if line.trim().is_empty() {
                break;
            }

            let Some(row) = Self::parse_table_row(line) else {
                break;
            };
            Self::normalize_table_cells(row, column_count);
            end += 1;
        }

        Some(end)
    }

    fn table_width_hint_from_cell(cell: &str) -> Option<Vec<usize>> {
        let start = cell.find(TABLE_WIDTH_MARKER_PREFIX)?;
        let marker_start = start + TABLE_WIDTH_MARKER_PREFIX.len();
        let marker_end = cell[marker_start..].find(HTML_COMMENT_SUFFIX)? + marker_start;
        let payload = &cell[marker_start..marker_end];

        let widths = payload
            .split(',')
            .map(|part| part.parse::<usize>().ok())
            .collect::<Option<Vec<_>>>()?;

        if widths.is_empty() {
            None
        } else {
            Some(widths)
        }
    }

    fn format_table_width_marker(widths: &[usize]) -> String {
        format!(
            "{}{}{}",
            TABLE_WIDTH_MARKER_PREFIX,
            widths
                .iter()
                .map(|width| width.to_string())
                .collect::<Vec<_>>()
                .join(","),
            HTML_COMMENT_SUFFIX
        )
    }
    fn parse_table_alignments(line: &str) -> Option<Vec<TableAlignment>> {
        let cells = Self::parse_table_row(line)?;
        let alignments = cells
            .into_iter()
            .map(|cell| {
                let trimmed = cell.trim();
                let left = trimmed.starts_with(':');
                let right = trimmed.ends_with(':');
                let without_left = trimmed.strip_prefix(':').unwrap_or(trimmed);
                let core = without_left.strip_suffix(':').unwrap_or(without_left);

                if core.len() < 3 || !core.chars().all(|ch| ch == '-') {
                    return None;
                }

                Some(match (left, right) {
                    (true, true) => TableAlignment::Center,
                    (true, false) => TableAlignment::Left,
                    (false, true) => TableAlignment::Right,
                    (false, false) => TableAlignment::None,
                })
            })
            .collect::<Option<Vec<_>>>()?;

        if alignments.is_empty() {
            None
        } else {
            Some(alignments)
        }
    }
    pub(crate) fn mark_table_header_with_width_hint(
        line: &str,
        width_hint: Option<&[usize]>,
    ) -> String {
        let Some(mut cells) = Self::parse_table_row(line) else {
            return line.to_string();
        };

        if let Some(first_cell) = cells.first_mut() {
            if Self::table_width_hint_from_cell(first_cell).is_none() {
                if let Some(widths) = width_hint {
                    first_cell.insert_str(0, &Self::format_table_width_marker(widths));
                }
            }
        }

        Self::format_table_row(&cells)
    }

    fn table_width_hint_from_cells(cells: &[String]) -> Option<Vec<usize>> {
        cells
            .first()
            .and_then(|cell| Self::table_width_hint_from_cell(cell))
    }
    fn parse_table_row(line: &str) -> Option<Vec<String>> {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.contains('|') {
            return None;
        }

        let mut content = trimmed;
        if let Some(stripped) = content.strip_prefix('|') {
            content = stripped;
        }
        if let Some(stripped) = content.strip_suffix('|') {
            content = stripped;
        }

        let mut cells = Vec::new();
        let mut current = String::new();
        let mut escape = false;

        for ch in content.chars() {
            if escape {
                current.push(ch);
                escape = false;
                continue;
            }

            match ch {
                '\\' => {
                    current.push(ch);
                    escape = true;
                }
                '|' => {
                    cells.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(ch),
            }
        }

        cells.push(current.trim().to_string());
        Some(cells)
    }

    fn wrap_table_rows(
        header: &[String],
        alignments: &[TableAlignment],
        body_rows: &[Vec<String>],
        wrap_width: usize,
        width_hint: Option<&[usize]>,
    ) -> Vec<String> {
        let widths = width_hint
            .map(|widths| {
                Self::normalize_table_width_hint(widths.to_vec(), header.len(), wrap_width)
            })
            .unwrap_or_else(|| Self::table_column_widths(header, body_rows, wrap_width));
        let is_block_continuation = header
            .first()
            .is_some_and(|cell| cell.contains(TABLE_BLOCK_CONTINUATION_MARKER));
        let wrapped_header = Self::wrap_table_row(header, &widths);
        let header_height = wrapped_header.iter().map(Vec::len).max().unwrap_or(1);
        let mut first_header_line = Self::table_row_line(&wrapped_header, 0);
        if let Some(first_cell) = first_header_line.first_mut() {
            first_cell.insert_str(0, &Self::format_table_width_marker(&widths));
            if is_block_continuation {
                first_cell.insert_str(0, TABLE_BLOCK_CONTINUATION_MARKER);
            }
        }

        let mut lines = vec![
            Self::format_table_row(&first_header_line),
            Self::format_table_delimiter(alignments, &widths),
        ];

        if !is_block_continuation {
            for line_index in 1..header_height {
                let mut continuation_cells = Self::table_row_line(&wrapped_header, line_index);
                if let Some(first_cell) = continuation_cells.first_mut() {
                    first_cell.insert_str(0, TABLE_CONTINUATION_MARKER);
                }
                lines.push(Self::format_table_row(&continuation_cells));
            }
        }

        for row in body_rows {
            let wrapped_cells = Self::wrap_table_row(row, &widths);

            let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
            for line_index in 0..row_height {
                let mut continuation_cells = Self::table_row_line(&wrapped_cells, line_index);
                if line_index > 0 {
                    if let Some(first_cell) = continuation_cells.first_mut() {
                        first_cell.insert_str(0, TABLE_CONTINUATION_MARKER);
                    }
                }
                lines.push(Self::format_table_row(&continuation_cells));
            }
        }

        lines
    }

    fn wrap_table_row(cells: &[String], widths: &[usize]) -> Vec<Vec<String>> {
        cells
            .iter()
            .zip(widths.iter().copied())
            .map(|(cell, width)| Self::wrap_table_cell(&Self::strip_table_markers(cell), width))
            .collect()
    }

    fn table_row_line(wrapped_cells: &[Vec<String>], line_index: usize) -> Vec<String> {
        wrapped_cells
            .iter()
            .map(|cell_lines| cell_lines.get(line_index).cloned().unwrap_or_default())
            .collect()
    }

    fn wrap_table_cell(cell: &str, width: usize) -> Vec<String> {
        if cell.is_empty() {
            return vec![String::new()];
        }

        textwrap::wrap(cell, textwrap::Options::new(width.max(1)))
            .into_iter()
            .map(|segment| segment.into_owned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
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
}
