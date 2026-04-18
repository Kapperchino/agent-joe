use textwrap::core::display_width;

pub struct DrawTable {}

const TABLE_CONTINUATION_MARKER: &str = "<!--__codex_table_continue__-->";
const TABLE_BLOCK_CONTINUATION_MARKER: &str = "<!--__codex_table_block_continue__-->";
const TABLE_WIDTH_MARKER_PREFIX: &str = "<!--__codex_table_widths__:";
const HTML_COMMENT_SUFFIX: &str = "-->";

#[derive(Clone, Copy)]
enum TableAlignment {
    Left,
    Right,
    Center,
    None,
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
        let header =
            Self::normalize_table_cells(Self::parse_table_row(*lines.get(start)?)?, column_count);

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
        let header = Self::normalize_table_cells(Self::parse_table_row(lines[0])?, column_count);
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

    fn wrap_plain_line(line: &str, wrap_width: usize) -> Vec<String> {
        if display_width(line) <= wrap_width {
            return vec![line.to_string()];
        }

        textwrap::wrap(line, textwrap::Options::new(wrap_width))
            .into_iter()
            .map(|segment| segment.into_owned())
            .collect()
    }

    pub(crate) fn wrap_markdown_tables(text: &str, wrap_width: usize) -> Vec<String> {
        let lines: Vec<&str> = text.split('\n').collect();
        let mut wrapped = Vec::new();
        let mut index = 0;
        let mut in_code = false;
        let mut fence_len = 0;

        while index < lines.len() {
            let line = lines[index];

            if !in_code {
                if let Some((consumed, table_lines)) =
                    Self::wrap_table_block(&lines[index..], wrap_width)
                {
                    wrapped.extend(table_lines);
                    index += consumed;
                    continue;
                }
            }

            if Self::is_code_fence(line, in_code.then_some(fence_len)) {
                let trimmed = line.trim();
                let backtick_count = trimmed.chars().take_while(|&c| c == '`').count();
                if in_code {
                    in_code = false;
                    fence_len = 0;
                } else {
                    in_code = true;
                    fence_len = backtick_count;
                }
            }

            wrapped.extend(Self::wrap_plain_line(line, wrap_width));
            index += 1;
        }

        wrapped
    }

    fn is_code_fence(line: &str, closing_fence_len: Option<usize>) -> bool {
        let trimmed = line.trim_start();
        let backtick_count = trimmed.chars().take_while(|&ch| ch == '`').count();
        if backtick_count < 3 {
            return false;
        }

        match closing_fence_len {
            Some(fence_len) => {
                let fully_trimmed = line.trim();
                backtick_count >= fence_len && fully_trimmed.len() == backtick_count
            }
            None => {
                let rest = trimmed[backtick_count..].trim();
                !rest.contains('`')
            }
        }
    }

    pub(crate) fn table_block_end(lines: &[&str], start: usize) -> Option<usize> {
        let alignments = Self::parse_table_alignments(*lines.get(start + 1)?)?;
        let column_count = alignments.len();
        Self::normalize_table_cells(Self::parse_table_row(*lines.get(start)?)?, column_count);

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

        let content = trimmed
            .strip_prefix('|')
            .unwrap_or(trimmed)
            .strip_suffix('|')
            .unwrap_or(trimmed);

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
        let mut lines = vec![
            Self::format_table_row(header),
            Self::format_table_delimiter(alignments, &widths),
        ];

        for row in body_rows {
            let wrapped_cells: Vec<Vec<String>> = row
                .iter()
                .zip(widths.iter().copied())
                .map(|(cell, width)| Self::wrap_table_cell(cell, width))
                .collect();

            let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
            for line_index in 0..row_height {
                let mut continuation_cells = wrapped_cells
                    .iter()
                    .map(|cell_lines| cell_lines.get(line_index).cloned().unwrap_or_default())
                    .collect::<Vec<_>>();
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
