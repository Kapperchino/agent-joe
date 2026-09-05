use crate::utils::draw_line::{CodeFence, DrawLine};
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

struct TableHeader {
    cells: Vec<String>,
    alignments: Vec<TableAlignment>,
}

impl TableHeader {
    fn parse(lines: &[&str]) -> Option<Self> {
        let alignments = DrawTable::parse_table_alignments(lines.get(1)?)?;
        let cells = DrawTable::parse_table_row(lines.first()?)?;
        (cells.len() == alignments.len()).then_some(Self { cells, alignments })
    }
}

impl DrawTable {
    pub(crate) fn mark_table_header_as_continuation(
        line: &str,
        width_hint: Option<&[usize]>,
    ) -> String {
        match Self::parse_table_row(line) {
            Some(mut cells) => {
                if let Some(first_cell) = cells.first_mut() {
                    if !first_cell.starts_with(TABLE_BLOCK_CONTINUATION_MARKER) {
                        first_cell.insert_str(0, TABLE_BLOCK_CONTINUATION_MARKER);
                    }
                    if Self::table_width_hint_from_cell(first_cell).is_none()
                        && let Some(widths) = width_hint
                    {
                        first_cell.insert_str(
                            TABLE_BLOCK_CONTINUATION_MARKER.len(),
                            &Self::format_table_width_marker(widths),
                        );
                    }
                }
                Self::format_table_row(&cells)
            }
            None => line.to_string(),
        }
    }

    pub fn table_block_spanning_split(lines: &[&str], split_line: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        let mut spanning_block = None;
        while start + 1 < lines.len() && spanning_block.is_none() {
            match Self::table_block_end(lines, start) {
                Some(end) => {
                    if start < split_line && split_line < end {
                        spanning_block = Some((start, end));
                    }
                    start = end.max(start + 1);
                }
                None => start += 1,
            }
        }
        spanning_block
    }

    pub(crate) fn table_width_hint(
        lines: &[&str],
        start: usize,
        end: usize,
        wrap_width: usize,
    ) -> Option<Vec<usize>> {
        let header = TableHeader::parse(lines.get(start..end)?)?;
        let column_count = header.cells.len();
        match Self::table_width_hint_from_cells(&header.cells) {
            Some(widths) => Some(Self::normalize_table_width_hint(
                widths,
                column_count,
                wrap_width,
            )),
            None => {
                let body_rows = lines[start + 2..end]
                    .iter()
                    .map(|line| {
                        Self::parse_table_row(line)
                            .map(|row| Self::normalize_table_cells(row, column_count))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::table_column_widths(
                    &header.cells,
                    &body_rows,
                    wrap_width,
                ))
            }
        }
    }

    fn wrap_table_block(lines: &[&str], wrap_width: usize) -> Option<(usize, Vec<String>)> {
        let header = TableHeader::parse(lines)?;
        let column_count = header.cells.len();
        let width_hint = Self::table_width_hint_from_cells(&header.cells)
            .map(|widths| Self::normalize_table_width_hint(widths, column_count, wrap_width));
        let body_rows = lines[2..]
            .iter()
            .map_while(|line| Self::parse_table_row(line))
            .map(|row| Self::normalize_table_cells(row, column_count))
            .collect::<Vec<_>>();
        let consumed = body_rows.len() + 2;
        let block = Self::wrap_table_rows(
            &header.cells,
            &header.alignments,
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
            vec![line.to_string()]
        } else {
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
        let mut code_fence: Option<CodeFence> = None;
        while index < lines.len() {
            let line = lines[index];
            let was_in_code = code_fence.is_some();
            let table = code_fence
                .is_none()
                .then(|| Self::wrap_table_block(&lines[index..], wrap_width))
                .flatten();
            match table {
                Some((consumed, table_lines)) => {
                    wrapped.extend(table_lines);
                    index += consumed;
                }
                None => {
                    match &code_fence {
                        Some(fence) if fence.is_closing(line) => code_fence = None,
                        None => code_fence = CodeFence::opening(line),
                        Some(_) => {}
                    }
                    if was_in_code || code_fence.is_some() {
                        wrapped.push(line.to_string());
                    } else {
                        wrapped.extend(Self::wrap_plain_line(line, wrap_width, true));
                    }
                    index += 1;
                }
            }
        }
        wrapped
    }

    pub(crate) fn table_block_end(lines: &[&str], start: usize) -> Option<usize> {
        let block = lines.get(start..)?;
        TableHeader::parse(block)?;
        Some(
            start
                + 2
                + block[2..]
                    .iter()
                    .map_while(|line| Self::parse_table_row(line))
                    .count(),
        )
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

                (core.len() >= 3 && core.chars().all(|ch| ch == '-')).then_some(
                    match (left, right) {
                        (true, true) => TableAlignment::Center,
                        (true, false) => TableAlignment::Left,
                        (false, true) => TableAlignment::Right,
                        (false, false) => TableAlignment::None,
                    },
                )
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
        match Self::parse_table_row(line) {
            Some(mut cells) => {
                if let Some(first_cell) = cells.first_mut()
                    && Self::table_width_hint_from_cell(first_cell).is_none()
                    && let Some(widths) = width_hint
                {
                    first_cell.insert_str(0, &Self::format_table_width_marker(widths));
                }
                Self::format_table_row(&cells)
            }
            None => line.to_string(),
        }
    }

    fn table_width_hint_from_cells(cells: &[String]) -> Option<Vec<usize>> {
        cells
            .first()
            .and_then(|cell| Self::table_width_hint_from_cell(cell))
    }
    fn parse_table_row(line: &str) -> Option<Vec<String>> {
        let trimmed = line.trim();
        trimmed.contains('|').then(|| {
            let content = trimmed.strip_prefix('|').unwrap_or(trimmed);
            let content = content.strip_suffix('|').unwrap_or(content);
            let mut cells = Vec::new();
            let mut current = String::new();
            let mut escape = false;
            for ch in content.chars() {
                match (escape, ch) {
                    (true, _) => {
                        current.push(ch);
                        escape = false;
                    }
                    (false, '\\') => {
                        current.push(ch);
                        escape = true;
                    }
                    (false, '|') => {
                        cells.push(current.trim().to_string());
                        current.clear();
                    }
                    (false, _) => current.push(ch),
                }
            }
            cells.push(current.trim().to_string());
            cells
        })
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
            vec![String::new()]
        } else {
            textwrap::wrap(cell, textwrap::Options::new(width.max(1)))
                .into_iter()
                .map(|segment| segment.into_owned())
                .collect()
        }
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
