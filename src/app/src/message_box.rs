use crate::draw_line::{DrawLine, RenderState};
use crate::draw_table::DrawTable;
use common_models::tui_models::State;
use markdown::mdast::Node;
use markdown::ParseOptions;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, StatefulWidget, Style};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::DefaultTerminal;
use throbber_widgets_tui::{Throbber, ThrobberState};

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
        DrawTable::wrap_markdown_tables(string, wrap_width)
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

        if DrawTable::wrap_markdown_tables(active_message, wrap_width).len() <= max_live_lines {
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
            .extend(DrawTable::wrap_markdown_tables(&prefix, wrap_width));
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

            if DrawTable::wrap_markdown_tables(&suffix, wrap_width).len() <= max_live_lines {
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
        if let Some((table_start, table_end)) =
            DrawTable::table_block_spanning_split(lines, split_line)
        {
            if split_line <= table_start + 2 {
                return None;
            }

            let width_hint = DrawTable::table_width_hint(lines, table_start, table_end, wrap_width);
            let mut prefix_lines = lines[..split_line]
                .iter()
                .map(|line| (*line).to_string())
                .collect::<Vec<_>>();
            if let Some(header) = prefix_lines.get_mut(table_start) {
                *header =
                    DrawTable::mark_table_header_with_width_hint(header, width_hint.as_deref());
            }

            let mut suffix_lines = Vec::with_capacity(lines.len().saturating_sub(split_line) + 2);
            suffix_lines.push(DrawTable::mark_table_header_as_continuation(
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
