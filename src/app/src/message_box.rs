use crate::draw_line::{DrawLine, RenderState};
use common_models::tui_models::State;
use markdown::mdast::Node;
use markdown::ParseOptions;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, StatefulWidget, Style};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::DefaultTerminal;
use textwrap::core::display_width;
use throbber_widgets_tui::{Throbber, ThrobberState};

#[derive(Clone, Copy)]
enum TableAlignment {
    Left,
    Right,
    Center,
    None,
}

const TABLE_CONTINUATION_MARKER: &str = "<!--__codex_table_continue__-->";
const TABLE_BLOCK_CONTINUATION_MARKER: &str = "<!--__codex_table_block_continue__-->";
const TABLE_WIDTH_MARKER_PREFIX: &str = "<!--__codex_table_widths__:";
const HTML_COMMENT_SUFFIX: &str = "-->";

pub enum Msg {
    Message(String),
    Tool(String),
    Empty,
}

pub struct MessageBox {}

pub struct MessageBoxState {
    messages: Vec<String>,
    msg_area_height: u16,
    msg_area_width: u16,
    scrollback_render_state: RenderState,
    active_message: Option<String>,
    draw_line: DrawLine,
    throbber_state: ThrobberState,
    pub actor_state: State,
    throbber_tick: usize,
}

impl MessageBoxState {
    pub fn new() -> MessageBoxState {
        MessageBoxState {
            messages: vec![],
            msg_area_height: 0,
            msg_area_width: 0,
            scrollback_render_state: Default::default(),
            active_message: None,
            draw_line: DrawLine::new(),
            throbber_state: Default::default(),
            actor_state: State::Ready,
            throbber_tick: 0,
        }
    }
    pub fn append(&mut self, msg: Msg) {
        self.messages.append(&mut self.wrap_msg(msg));
    }

    pub fn pop(&mut self) {
        self.messages.pop();
    }

    pub fn get_last(&self) -> Option<String> {
        self.messages.last().cloned()
    }

    pub fn update_width_height(&mut self, width: u16, height: u16) {
        self.msg_area_height = height;
        self.msg_area_width = width;
    }

    fn wrap_msg(&self, msg: Msg) -> Vec<String> {
        match msg {
            Msg::Message(str) => self.wrap_str(&str),
            Msg::Tool(str) => self.wrap_tool_str(&str),
            Msg::Empty => vec![String::new()],
        }
    }

    fn wrap_str(&self, string: &str) -> Vec<String> {
        let wrap_width = self.msg_area_width.saturating_sub(2).max(1) as usize;
        Self::wrap_markdown_tables(string, wrap_width)
    }

    fn wrap_tool_str(&self, string: &str) -> Vec<String> {
        if let Some(content) = string.strip_prefix("- ") {
            let wrap_width = self.msg_area_width.saturating_sub(2).max(1);
            return textwrap::wrap(
                content,
                textwrap::Options::new(wrap_width as usize)
                    .initial_indent("- ")
                    .subsequent_indent("  "),
            )
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        }

        self.wrap_str(string)
    }
    fn output_lines(&self) -> Vec<Line<'static>> {
        let mut render_state = self.scrollback_render_state.clone();
        let mut lines = self
            .draw_line
            .render_lines_with_state(&self.messages, &mut render_state);
        if let Some(active_message) = &self.active_message {
            let active_lines = self.wrap_str(active_message);
            lines.extend(
                self.draw_line
                    .render_lines_with_state(&active_lines, &mut render_state),
            );
        }
        match self.actor_state {
            State::ThinkingStart => {
                lines.push(
                    Self::thinking_throbber("thinking".to_string()).to_line(&self.throbber_state),
                );
            }
            State::ToolStart => {
                lines.push(
                    Self::thinking_throbber("working".to_string()).to_line(&self.throbber_state),
                );
            }
            _ => {}
        };
        let visible_limit = self.max_live_messages();
        if lines.len() > visible_limit {
            lines = lines.split_off(lines.len().saturating_sub(visible_limit));
        }
        lines
    }

    fn max_live_messages(&self) -> usize {
        let throbber_reserved = usize::from(matches!(self.actor_state, State::ThinkingStart));
        (self.msg_area_height as usize).saturating_sub(throbber_reserved)
    }

    pub(crate) fn flush_scrollback(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> color_eyre::Result<()> {
        self.compact_active_message();

        let active_line_count = self.active_message_line_count();
        let max_live_messages = self.max_live_messages().saturating_sub(active_line_count);
        if self.messages.len() <= max_live_messages {
            return Ok(());
        }

        let flush_count = Self::adjust_flush_count_for_tables(
            &self.messages,
            self.messages.len().saturating_sub(max_live_messages),
        );
        let flushed_lines = self.messages.drain(0..flush_count).collect::<Vec<_>>();
        let rendered_lines = self
            .draw_line
            .render_lines_with_state(&flushed_lines, &mut self.scrollback_render_state);

        terminal.insert_before(rendered_lines.len() as u16, |buf| {
            Paragraph::new(rendered_lines).render(buf.area, buf);
        })?;

        Ok(())
    }

    fn compact_active_message(&mut self) {
        let max_live_lines = self.max_live_messages();
        if max_live_lines == 0 {
            return;
        }

        let wrap_width = self.msg_area_width.saturating_sub(2).max(1) as usize;
        let Some(active_message) = self.active_message.as_ref() else {
            return;
        };

        if Self::wrap_markdown_tables(active_message, wrap_width).len() <= max_live_lines {
            return;
        }

        let Some((prefix, suffix)) =
            Self::split_stream_message_for_overflow(active_message, max_live_lines, wrap_width)
        else {
            return;
        };

        if prefix.is_empty() {
            return;
        }

        self.messages
            .extend(Self::wrap_markdown_tables(&prefix, wrap_width));
        self.active_message = Some(suffix);
    }

    pub fn start_stream_message(&mut self, leading_blank_line: bool) {
        if leading_blank_line {
            self.append(Msg::Empty);
        }
        self.active_message = Some(String::new());
    }

    pub fn push_stream_message(&mut self, chunk: &str) {
        self.active_message
            .get_or_insert_with(String::new)
            .push_str(chunk);
    }

    pub fn finish_stream_message(&mut self, trailing_blank_line: bool) {
        if let Some(message) = self.active_message.take() {
            if !message.is_empty() {
                self.messages.extend(self.wrap_str(&message));
            }
        }

        if trailing_blank_line {
            self.append(Msg::Empty);
        }
    }

    fn adjust_flush_count_for_tables(lines: &[String], flush_count: usize) -> usize {
        if flush_count == 0 || flush_count >= lines.len() {
            return flush_count;
        }

        let markdown = lines.join("\n");
        let tree = match markdown::to_mdast(&markdown, &Self::markdown_parse_options()) {
            Ok(tree) => tree,
            Err(_) => return flush_count,
        };

        Self::table_flush_boundary(&tree, flush_count)
            .unwrap_or(flush_count)
            .min(lines.len())
    }

    fn table_flush_boundary(node: &Node, flush_count: usize) -> Option<usize> {
        let boundary = match node {
            Node::Table(table) => {
                let table_start_line = table
                    .position
                    .as_ref()
                    .map(|position| position.start.line)
                    .unwrap_or(0);
                let table_end_line = table
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        Node::TableRow(row) => row.position.as_ref().map(Self::table_end_line),
                        _ => None,
                    })
                    .max()
                    .or_else(|| table.position.as_ref().map(Self::table_end_line));

                table_end_line.and_then(|table_end_line| {
                    if table_start_line <= flush_count && flush_count < table_end_line {
                        Some(table_end_line)
                    } else {
                        None
                    }
                })
            }
            _ => None,
        };

        let child_boundary = node.children().and_then(|children| {
            children
                .iter()
                .filter_map(|child| Self::table_flush_boundary(child, flush_count))
                .max()
        });

        boundary.into_iter().chain(child_boundary).max()
    }

    fn markdown_parse_options() -> ParseOptions {
        let mut options = ParseOptions::default();
        options.constructs.gfm_table = true;
        options
    }

    fn table_end_line(position: &markdown::unist::Position) -> usize {
        if position.end.column == 1 {
            position.end.line.saturating_sub(1)
        } else {
            position.end.line
        }
    }

    fn active_message_line_count(&self) -> usize {
        self.active_message
            .as_ref()
            .map(|message| self.wrap_str(message).len())
            .unwrap_or(0)
    }

    fn split_stream_message_for_overflow(
        message: &str,
        max_live_lines: usize,
        wrap_width: usize,
    ) -> Option<(String, String)> {
        if max_live_lines == 0 {
            return None;
        }

        let lines: Vec<&str> = message.split('\n').collect();
        if lines.len() < 2 {
            return None;
        }

        for split_line in 1..lines.len() {
            let Some((prefix, suffix)) =
                Self::split_stream_message_at_line(&lines, split_line, wrap_width)
            else {
                continue;
            };

            if suffix.is_empty() {
                continue;
            }

            if Self::wrap_markdown_tables(&suffix, wrap_width).len() <= max_live_lines {
                return Some((prefix, suffix));
            }
        }

        None
    }

    fn split_stream_message_at_line(
        lines: &[&str],
        split_line: usize,
        wrap_width: usize,
    ) -> Option<(String, String)> {
        if !(1..lines.len()).contains(&split_line) {
            return None;
        }

        if let Some((table_start, table_end)) = Self::table_block_spanning_split(lines, split_line)
        {
            if split_line <= table_start + 2 {
                return None;
            }

            let width_hint = Self::table_width_hint(lines, table_start, table_end, wrap_width);
            let mut prefix_lines = lines[..split_line]
                .iter()
                .map(|line| (*line).to_string())
                .collect::<Vec<_>>();
            if let Some(header) = prefix_lines.get_mut(table_start) {
                *header = Self::mark_table_header_with_width_hint(header, width_hint.as_deref());
            }

            let mut suffix_lines = Vec::with_capacity(lines.len().saturating_sub(split_line) + 2);
            suffix_lines.push(Self::mark_table_header_as_continuation(
                lines[table_start],
                width_hint.as_deref(),
            ));
            suffix_lines.push(lines[table_start + 1].to_string());
            suffix_lines.extend(lines[split_line..].iter().map(|line| (*line).to_string()));
            return Some((prefix_lines.join("\n"), suffix_lines.join("\n")));
        }

        Some((
            lines[..split_line].join("\n"),
            lines[split_line..].join("\n"),
        ))
    }

    fn table_block_spanning_split(lines: &[&str], split_line: usize) -> Option<(usize, usize)> {
        let mut start = 0;
        while start + 1 < lines.len() {
            let Some(end) = Self::table_block_end(lines, start) else {
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

    fn table_block_end(lines: &[&str], start: usize) -> Option<usize> {
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

    fn table_width_hint(
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

    fn wrap_markdown_tables(text: &str, wrap_width: usize) -> Vec<String> {
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

    fn wrap_plain_line(line: &str, wrap_width: usize) -> Vec<String> {
        if display_width(line) <= wrap_width {
            return vec![line.to_string()];
        }

        textwrap::wrap(line, textwrap::Options::new(wrap_width))
            .into_iter()
            .map(|segment| segment.into_owned())
            .collect()
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

    fn wrap_table_cell(cell: &str, width: usize) -> Vec<String> {
        if cell.is_empty() {
            return vec![String::new()];
        }

        textwrap::wrap(cell, textwrap::Options::new(width.max(1)))
            .into_iter()
            .map(|segment| segment.into_owned())
            .collect()
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
                    TableAlignment::Left => format!(":{}", "-".repeat(width.saturating_sub(1))),
                    TableAlignment::Right => format!("{}:", "-".repeat(width.saturating_sub(1))),
                    TableAlignment::Center => {
                        format!(":{}:", "-".repeat(width.saturating_sub(2).max(1)))
                    }
                    TableAlignment::None => "-".repeat(width),
                }
            })
            .collect::<Vec<_>>();

        Self::format_table_row(&cells)
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

    fn normalize_table_cells(mut cells: Vec<String>, column_count: usize) -> Vec<String> {
        cells.truncate(column_count);
        while cells.len() < column_count {
            cells.push(String::new());
        }
        cells
    }

    fn mark_table_header_with_width_hint(line: &str, width_hint: Option<&[usize]>) -> String {
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

    fn mark_table_header_as_continuation(line: &str, width_hint: Option<&[usize]>) -> String {
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

    fn strip_table_markers(cell: &str) -> String {
        Self::strip_table_width_markers(
            &cell
                .replace(TABLE_CONTINUATION_MARKER, "")
                .replace(TABLE_BLOCK_CONTINUATION_MARKER, ""),
        )
    }

    fn table_width_hint_from_cells(cells: &[String]) -> Option<Vec<usize>> {
        cells
            .first()
            .and_then(|cell| Self::table_width_hint_from_cell(cell))
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

    fn thinking_throbber(label: String) -> Throbber<'static> {
        Throbber::default()
            .label(label)
            .style(Style::default().fg(Color::Yellow))
            .throbber_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
    }

    pub fn advance_throbber(&mut self) {
        if !matches!(self.actor_state, State::ThinkingStart) {
            self.throbber_state = ThrobberState::default();
            self.throbber_tick = 0;
            return;
        }

        self.throbber_tick = self.throbber_tick.wrapping_add(1);
        if self.throbber_tick % 8 == 0 {
            self.throbber_state.calc_next();
        }
    }
}

impl StatefulWidget for MessageBox {
    type State = MessageBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let messages = Paragraph::new(state.output_lines());
        messages.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageBoxState, Msg, TABLE_BLOCK_CONTINUATION_MARKER, TABLE_WIDTH_MARKER_PREFIX};

    fn line_text(line: &ratatui::prelude::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn extends_flush_past_split_table_boundary() {
        let lines = vec![
            "| Name | Score |".to_string(),
            "| ---- | ----: |".to_string(),
            "| Ada  | 10    |".to_string(),
            "After".to_string(),
        ];

        assert_eq!(MessageBoxState::adjust_flush_count_for_tables(&lines, 2), 4);
    }

    #[test]
    fn leaves_flush_count_when_not_inside_table() {
        let lines = vec![
            "Intro".to_string(),
            "| Name | Score |".to_string(),
            "| ---- | ----: |".to_string(),
            "| Ada  | 10    |".to_string(),
        ];

        assert_eq!(MessageBoxState::adjust_flush_count_for_tables(&lines, 1), 1);
        assert_eq!(MessageBoxState::adjust_flush_count_for_tables(&lines, 3), 4);
    }

    #[test]
    fn wraps_long_table_cells_as_continuation_rows() {
        let mut state = MessageBoxState::new();
        state.update_width_height(56, 20);
        state.append(Msg::Message(
            [
                "| Part | What it does |",
                "| ---- | ------------ |",
                "| Worker actor | Main runtime actor that coordinates tool usage. |",
            ]
            .join("\n"),
        ));

        let rendered = state.output_lines();
        let rendered_text: Vec<String> = rendered.iter().map(line_text).collect();

        assert!(
            rendered_text
                .iter()
                .any(|line| line.starts_with("             │ ")),
            "expected wrapped continuation line in second column: {rendered_text:?}"
        );
        assert!(
            !rendered_text
                .iter()
                .any(|line| line.contains("coordinates tool usage. │")),
            "unexpected broken first-column continuation: {rendered_text:?}"
        );
    }

    #[test]
    fn streaming_updates_keep_table_rows_intact() {
        let mut state = MessageBoxState::new();
        state.update_width_height(90, 20);
        state.start_stream_message(true);
        state.push_stream_message(
            "| Aspect | What src/actors/src/actor.rs does |\n| ------ | ---------------------------------- |\n| Main role | Implements the main Worker actor that drives a chat/task session. |\n| Message handling | In handle, it reacts to incoming messages like starting work, commands, stream",
        );
        state.push_stream_message(
            " items, file changes, and tool usage. |\n| Streaming | Processes streamed LLM output and decides what to do next as chunks arrive. |",
        );

        let rendered = state.output_lines();
        let rendered_text: Vec<String> = rendered.iter().map(line_text).collect();

        assert!(
            rendered_text
                .iter()
                .any(|line| line.contains("Message handling")),
            "expected rendered message-handling row: {rendered_text:?}"
        );
        assert!(
            rendered_text
                .iter()
                .any(|line| line.starts_with("                 │ ")),
            "expected continuation row for wrapped cell: {rendered_text:?}"
        );
        assert!(
            rendered_text.iter().any(|line| line.contains("Streaming")),
            "expected later rows to remain in table: {rendered_text:?}"
        );
        assert!(
            !rendered_text
                .iter()
                .any(|line| line.starts_with("| Streaming |")),
            "unexpected raw markdown row leaked into output: {rendered_text:?}"
        );
    }

    #[test]
    fn row_separators_follow_logical_rows_not_wrapped_lines() {
        let mut state = MessageBoxState::new();
        state.update_width_height(56, 20);
        state.append(Msg::Message(
            [
                "| Aspect | What it does |",
                "| ------ | ------------ |",
                "| Message handling | Handles incoming messages like starting work, commands, stream items, file changes, and tool usage. |",
                "| Streaming | Processes streamed LLM output. |",
            ]
            .join("\n"),
        ));

        let rendered = state.output_lines();
        let rendered_text: Vec<String> = rendered.iter().map(line_text).collect();
        let separator_count = rendered_text
            .iter()
            .filter(|line| line.contains('┼'))
            .count();

        assert_eq!(
            separator_count, 2,
            "unexpected separators: {rendered_text:?}"
        );
        assert!(
            rendered_text
                .iter()
                .any(|line| line.starts_with("                 │ ")),
            "expected wrapped continuation line: {rendered_text:?}"
        );
    }

    #[test]
    fn compacts_streaming_tables_before_message_stop() {
        let mut state = MessageBoxState::new();
        state.update_width_height(72, 6);
        state.start_stream_message(true);
        state.push_stream_message(
            [
                "| Aspect | What it does |",
                "| ------ | ------------ |",
                "| Main role | Implements the main Worker actor. |",
                "| Startup | Initializes dependencies and actor state. |",
                "| Message handling | Processes incoming messages. |",
                "| Streaming | Handles incremental LLM output. |",
                "| Tool orchestration | Runs requested tools. |",
                "| TUI communication | Sends updates to the UI. |",
            ]
            .join("\n")
            .as_str(),
        );

        state.compact_active_message();

        let active_message = state.active_message.clone().unwrap_or_default();

        assert!(
            state.messages.iter().any(|line| line.contains("Main role")),
            "expected streamed prefix to be committed early: {:?}",
            state.messages
        );
        assert!(
            active_message.starts_with(&format!("| {TABLE_BLOCK_CONTINUATION_MARKER}"))
                && active_message.contains(TABLE_WIDTH_MARKER_PREFIX)
                && active_message.contains("Aspect | What it does |\n| ------ | ------------ |"),
            "expected active suffix to remain a valid table: {active_message:?}"
        );
        assert!(
            active_message.contains("| Tool orchestration | Runs requested tools. |"),
            "expected later rows to remain active: {active_message:?}"
        );
        assert!(
            !active_message.contains("| Main role | Implements the main Worker actor. |"),
            "expected early rows to move into committed messages: {active_message:?}"
        );
    }

    #[test]
    fn streaming_table_compaction_does_not_repeat_headers() {
        let mut state = MessageBoxState::new();
        state.update_width_height(72, 6);
        state.start_stream_message(true);
        state.push_stream_message(
            [
                "| Aspect | What it does |",
                "| ------ | ------------ |",
                "| Main role | Implements the main Worker actor. |",
                "| Startup | Initializes dependencies and actor state. |",
                "| Message handling | Processes incoming messages. |",
                "| Streaming | Handles incremental LLM output. |",
                "| Tool orchestration | Runs requested tools. |",
                "| TUI communication | Sends updates to the UI. |",
            ]
            .join("\n")
            .as_str(),
        );

        state.compact_active_message();
        state.update_width_height(90, 20);

        let rendered = state.output_lines();
        let rendered_text: Vec<String> = rendered.iter().map(line_text).collect();
        let header_count = rendered_text
            .iter()
            .filter(|line| line.contains("Aspect") && line.contains("What it does"))
            .count();

        assert_eq!(
            header_count, 1,
            "expected table header to render once: {rendered_text:?}"
        );
    }

    #[test]
    fn streaming_table_compaction_preserves_column_alignment() {
        let mut state = MessageBoxState::new();
        state.update_width_height(72, 6);
        state.start_stream_message(true);
        state.push_stream_message(
            [
                "| Part | What it does |",
                "| ---- | ------------ |",
                "| Worker actor | Main orchestration actor for chat, tool calls, and TUI updates. |",
                "| Dependency | Injects the LLM client, available tools, TUI sender, and debug flag. |",
                "| pre_start | Builds CurContext, then spawns linked CacheActor and FileActor, and creates ActorState. |",
                "| Message enum | Defines actor inputs: StartWork, Command, UseTool, Noop, ProcessStreamItem, KYS. |",
                "| StartWork | Optionally appends the user prompt to history, builds an LLM request, starts streaming, and pumps stream events back into the actor. |",
            ]
            .join("\n")
            .as_str(),
        );

        state.compact_active_message();
        state.update_width_height(96, 20);

        let rendered = state.output_lines();
        let rendered_text: Vec<String> = rendered.iter().map(line_text).collect();
        let split_positions: Vec<usize> = rendered_text
            .iter()
            .filter_map(|line| line.chars().position(|ch| ch == '│' || ch == '┼'))
            .collect();

        assert!(
            split_positions.len() >= 4,
            "expected multiple rendered table rows: {rendered_text:?}"
        );
        assert!(
            split_positions
                .iter()
                .all(|position| *position == split_positions[0]),
            "expected consistent column alignment across compacted segments: {rendered_text:?}"
        );
    }
}
