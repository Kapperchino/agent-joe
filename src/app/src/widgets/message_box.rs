use crate::utils::draw_line::{DrawLine, RenderState};
use crate::utils::draw_table::DrawTable;
use common_models::tui_models::State;
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use markdown::ParseOptions;
use markdown::mdast::Node;
use ratatui::DefaultTerminal;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, StatefulWidget, Style};
use ratatui::widgets::{Paragraph, Widget};
use throbber_widgets_tui::{Throbber, ThrobberState};

const HORIZONTAL_PADDING: u16 = 2;
const MIN_WRAP_WIDTH: usize = 1;
const TOOL_SUMMARY_PREFIX: &str = "- ";
const TOOL_SUMMARY_CONTINUATION_INDENT: &str = "  ";
const THROBBER_FRAME_TICKS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Message(String),
    Tool(String),
    Empty,
}

pub struct MessageBox {}

pub struct MessageBoxState {
    viewport: MessageViewport,
    messages: MessageBuffer,
    scrollback: ScrollbackRenderer,
    busy_indicator: BusyIndicator,
    pub actor_state: State,
}

impl MessageBoxState {
    pub fn new() -> MessageBoxState {
        MessageBoxState {
            viewport: MessageViewport::default(),
            messages: MessageBuffer::default(),
            scrollback: ScrollbackRenderer::new(),
            busy_indicator: BusyIndicator::default(),
            actor_state: State::Ready,
        }
    }

    pub fn append(&mut self, msg: Msg) {
        let formatter = self.formatter();
        self.messages.append(msg, &formatter);
    }

    pub fn pop(&mut self) {
        self.messages.pop();
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.scrollback.reset();
        self.busy_indicator.reset();
    }

    pub fn get_last(&self) -> Option<String> {
        self.messages.last().cloned()
    }

    pub fn update_width_height(&mut self, width: u16, height: u16) {
        self.viewport.update(width, height);
    }

    pub(crate) fn flush_scrollback(
        &mut self,
        terminal: &mut DefaultTerminal,
        do_clear: bool,
    ) -> color_eyre::Result<()> {
        if do_clear {
            self.clear_terminal(terminal)?;
            return Ok(());
        }

        let formatter = self.formatter();
        self.messages
            .compact_active(self.max_live_messages(), &formatter);

        let active_line_count = self.messages.active_line_count(&formatter);
        let committed_capacity = self.max_live_messages().saturating_sub(active_line_count);
        if self.messages.len() <= committed_capacity {
            return Ok(());
        }

        let requested_flush = self.messages.len().saturating_sub(committed_capacity);
        let flush_count =
            TableFlushBoundary::adjusted_flush_count(self.messages.lines(), requested_flush);
        let flushed_lines = self.messages.drain_front(flush_count);
        let rendered_lines = self.scrollback.render_flushed_lines(&flushed_lines);

        terminal.insert_before(rendered_lines.len() as u16, |buf| {
            Paragraph::new(rendered_lines).render(buf.area, buf);
        })?;

        Ok(())
    }

    pub fn start_stream_message(&mut self, leading_blank_line: bool) {
        let formatter = self.formatter();
        self.messages
            .start_stream_message(leading_blank_line, &formatter);
    }

    pub fn push_stream_message(&mut self, chunk: &str) {
        self.messages.push_stream_chunk(chunk);
    }

    pub fn finish_stream_message(&mut self, trailing_blank_line: bool) {
        let formatter = self.formatter();
        self.messages
            .finish_stream_message(trailing_blank_line, &formatter);
    }

    pub fn advance_throbber(&mut self) {
        self.busy_indicator.advance(&self.actor_state);
    }

    fn formatter(&self) -> MessageFormatter {
        MessageFormatter::new(self.viewport.wrap_width())
    }

    fn clear_terminal(&mut self, terminal: &mut DefaultTerminal) -> color_eyre::Result<()> {
        self.scrollback.reset();
        execute!(
            terminal.backend_mut(),
            MoveTo(0, 0),
            Clear(ClearType::All),
            Clear(ClearType::Purge),
        )?;
        terminal.clear()?;
        Ok(())
    }

    fn output_lines(&self) -> Vec<Line<'static>> {
        let formatter = self.formatter();
        let lines = self.scrollback.render_live_lines(
            &self.messages,
            &formatter,
            &self.actor_state,
            &self.busy_indicator,
        );
        self.viewport.visible_lines(lines)
    }

    fn max_live_messages(&self) -> usize {
        self.viewport.message_line_capacity(&self.actor_state)
    }
}

impl Default for MessageBoxState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MessageViewport {
    width: u16,
    height: u16,
}

impl MessageViewport {
    fn update(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    fn wrap_width(&self) -> usize {
        self.width
            .saturating_sub(HORIZONTAL_PADDING)
            .max(MIN_WRAP_WIDTH as u16) as usize
    }

    fn message_line_capacity(&self, actor_state: &State) -> usize {
        usize::from(self.height).saturating_sub(BusyIndicator::reserved_lines(actor_state))
    }

    fn visible_lines(&self, mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        let capacity = usize::from(self.height);
        if lines.len() > capacity {
            lines.split_off(lines.len().saturating_sub(capacity))
        } else {
            lines
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MessageFormatter {
    wrap_width: usize,
}

impl MessageFormatter {
    fn new(wrap_width: usize) -> Self {
        Self { wrap_width }
    }

    fn wrap_msg(&self, msg: Msg) -> Vec<String> {
        match msg {
            Msg::Message(message) => self.wrap_message(&message),
            Msg::Tool(message) => self.wrap_tool_message(&message),
            Msg::Empty => vec![String::new()],
        }
    }

    fn wrap_message(&self, message: &str) -> Vec<String> {
        DrawTable::wrap_markdown_tables(message, self.wrap_width)
    }

    fn wrap_tool_message(&self, message: &str) -> Vec<String> {
        let Some(content) = message.strip_prefix(TOOL_SUMMARY_PREFIX) else {
            return self.wrap_message(message);
        };

        match content.split_once('\n') {
            Some((summary, rest)) => self
                .wrap_tool_summary(summary)
                .into_iter()
                .chain(rest.split('\n').map(str::to_string))
                .collect(),
            None => self.wrap_tool_summary(content),
        }
    }

    fn wrap_tool_summary(&self, summary: &str) -> Vec<String> {
        textwrap::wrap(
            summary,
            textwrap::Options::new(self.wrap_width)
                .initial_indent(TOOL_SUMMARY_PREFIX)
                .subsequent_indent(TOOL_SUMMARY_CONTINUATION_INDENT),
        )
        .into_iter()
        .map(|line| line.into_owned())
        .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct MessageBuffer {
    committed: Vec<String>,
    active: Option<String>,
}

impl MessageBuffer {
    fn append(&mut self, msg: Msg, formatter: &MessageFormatter) {
        self.committed.extend(formatter.wrap_msg(msg));
    }

    fn pop(&mut self) {
        self.committed.pop();
    }

    fn clear(&mut self) {
        self.committed.clear();
        self.active = None;
    }

    fn last(&self) -> Option<&String> {
        self.committed.last()
    }

    fn len(&self) -> usize {
        self.committed.len()
    }

    fn lines(&self) -> &[String] {
        &self.committed
    }

    fn active_text(&self) -> Option<&str> {
        self.active.as_deref()
    }

    fn active_line_count(&self, formatter: &MessageFormatter) -> usize {
        self.active_text()
            .map(|message| formatter.wrap_message(message).len())
            .unwrap_or(0)
    }

    fn drain_front(&mut self, count: usize) -> Vec<String> {
        self.committed
            .drain(0..count.min(self.committed.len()))
            .collect()
    }

    fn start_stream_message(&mut self, leading_blank_line: bool, formatter: &MessageFormatter) {
        if leading_blank_line {
            self.append(Msg::Empty, formatter);
        }
        self.active = Some(String::new());
    }

    fn push_stream_chunk(&mut self, chunk: &str) {
        self.active.get_or_insert_with(String::new).push_str(chunk);
    }

    fn finish_stream_message(&mut self, trailing_blank_line: bool, formatter: &MessageFormatter) {
        if let Some(message) = self.active.take() {
            if !message.is_empty() {
                self.committed.extend(formatter.wrap_message(&message));
            }
        }

        if trailing_blank_line {
            self.append(Msg::Empty, formatter);
        }
    }

    fn compact_active(&mut self, max_live_lines: usize, formatter: &MessageFormatter) {
        if max_live_lines == 0 {
            return;
        }

        let Some(active_message) = self.active_text() else {
            return;
        };

        if formatter.wrap_message(active_message).len() <= max_live_lines {
            return;
        }

        let Some(split) =
            StreamOverflowSplitter::split(active_message, max_live_lines, formatter.wrap_width)
        else {
            return;
        };

        if split.prefix.is_empty() {
            return;
        }

        self.committed.extend(formatter.wrap_message(&split.prefix));
        self.active = Some(split.suffix);
    }
}

struct ScrollbackRenderer {
    draw_line: DrawLine,
    state: RenderState,
}

impl ScrollbackRenderer {
    fn new() -> Self {
        Self {
            draw_line: DrawLine::new(),
            state: RenderState::default(),
        }
    }

    fn reset(&mut self) {
        self.state = RenderState::default();
    }

    fn render_flushed_lines(&mut self, lines: &[String]) -> Vec<Line<'static>> {
        self.draw_line
            .render_lines_with_state(lines, &mut self.state)
    }

    fn render_live_lines(
        &self,
        messages: &MessageBuffer,
        formatter: &MessageFormatter,
        actor_state: &State,
        busy_indicator: &BusyIndicator,
    ) -> Vec<Line<'static>> {
        let mut render_state = self.state.clone();
        let mut lines = self
            .draw_line
            .render_lines_with_state(messages.lines(), &mut render_state);

        if let Some(active_message) = messages.active_text() {
            let active_lines = formatter.wrap_message(active_message);
            lines.extend(
                self.draw_line
                    .render_lines_with_state(&active_lines, &mut render_state),
            );
        }

        if let Some(line) = busy_indicator.render_line(actor_state) {
            lines.push(line);
        }

        lines
    }
}

#[derive(Default)]
struct BusyIndicator {
    state: ThrobberState,
    ticks: usize,
}

impl BusyIndicator {
    fn reserved_lines(actor_state: &State) -> usize {
        usize::from(Self::label(actor_state).is_some())
    }

    fn advance(&mut self, actor_state: &State) {
        if !Self::should_tick(actor_state) {
            self.reset();
            return;
        }

        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % THROBBER_FRAME_TICKS == 0 {
            self.state.calc_next();
        }
    }

    fn reset(&mut self) {
        self.state = ThrobberState::default();
        self.ticks = 0;
    }

    fn render_line(&self, actor_state: &State) -> Option<Line<'static>> {
        Self::label(actor_state).map(|label| Self::throbber(label).to_line(&self.state))
    }

    fn label(actor_state: &State) -> Option<&'static str> {
        match actor_state {
            State::ThinkingStart => Some("thinking"),
            State::ToolStart => Some("working"),
            _ => None,
        }
    }

    fn should_tick(actor_state: &State) -> bool {
        matches!(
            actor_state,
            State::ThinkingStart | State::ToolStart | State::ThinkingStop | State::ToolStop
        )
    }

    fn throbber(label: &str) -> Throbber<'static> {
        Throbber::default()
            .label(label.to_string())
            .style(Style::default().fg(Color::Yellow))
            .throbber_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StreamSplit {
    prefix: String,
    suffix: String,
}

struct StreamOverflowSplitter;

impl StreamOverflowSplitter {
    fn split(message: &str, max_live_lines: usize, wrap_width: usize) -> Option<StreamSplit> {
        if max_live_lines == 0 {
            return None;
        }

        let lines = message.split('\n').collect::<Vec<_>>();
        if lines.len() < 2 {
            return None;
        }

        for split_line in 1..lines.len() {
            let Some(split) = Self::split_at_line(&lines, split_line, wrap_width) else {
                continue;
            };

            if split.suffix.is_empty() {
                continue;
            }

            if DrawTable::wrap_markdown_tables(&split.suffix, wrap_width).len() <= max_live_lines {
                return Some(split);
            }
        }

        None
    }

    fn split_at_line(lines: &[&str], split_line: usize, wrap_width: usize) -> Option<StreamSplit> {
        if !(1..lines.len()).contains(&split_line) {
            return None;
        }

        if let Some((table_start, table_end)) =
            DrawTable::table_block_spanning_split(lines, split_line)
        {
            return Self::split_table_at_line(
                lines,
                split_line,
                table_start,
                table_end,
                wrap_width,
            );
        }

        Some(StreamSplit {
            prefix: lines[..split_line].join("\n"),
            suffix: lines[split_line..].join("\n"),
        })
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
}

struct TableFlushBoundary;

impl TableFlushBoundary {
    fn adjusted_flush_count(lines: &[String], flush_count: usize) -> usize {
        if flush_count == 0 || flush_count >= lines.len() {
            return flush_count;
        }

        let markdown = lines.join("\n");
        let tree = match markdown::to_mdast(&markdown, &Self::markdown_parse_options()) {
            Ok(tree) => tree,
            Err(_) => return flush_count,
        };

        Self::table_boundary(&tree, flush_count)
            .unwrap_or(flush_count)
            .min(lines.len())
    }

    fn table_boundary(node: &Node, flush_count: usize) -> Option<usize> {
        let node_boundary = match node {
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
                .filter_map(|child| Self::table_boundary(child, flush_count))
                .max()
        });

        node_boundary.into_iter().chain(child_boundary).max()
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
}

impl StatefulWidget for MessageBox {
    type State = MessageBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let messages = Paragraph::new(state.output_lines());
        messages.render(area, buf);
    }
}