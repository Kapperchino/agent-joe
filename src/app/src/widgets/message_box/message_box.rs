use crate::widgets::message_box::format::MessageFormatter;
use crate::widgets::message_box::indicator::BusyIndicator;
use crate::widgets::message_box::scrollback::ScrollbackRenderer;
use crate::widgets::message_box::transcript::MessageTranscript;
use crate::widgets::message_box::viewport::MessageViewport;
use crate::{theme, widgets::welcome::Welcome};
use common_models::tui_models::State;
use crossterm::cursor::MoveTo;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, StatefulWidget};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{DefaultTerminal, Terminal, backend::Backend};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Message(String),
    Tool(String),
    Empty,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ToolDisplay {
    #[default]
    Grouped,
    Expanded,
}

pub struct MessageBox {}

pub struct MessageBoxState {
    view: ConversationView,
    viewport: MessageViewport,
    transcript: MessageTranscript,
    tool_history: ToolHistoryView,
    scrollback: ScrollbackRenderer,
    busy_indicator: BusyIndicator,
    pub actor_state: State,
}

enum ConversationView {
    Welcome,
    Transcript,
}

#[derive(Default, Clone, Copy)]
enum ToolHistoryView {
    #[default]
    Collapsed,
    Expanded {
        offset: usize,
    },
}

impl MessageBoxState {
    pub fn new() -> MessageBoxState {
        Self::with_tool_display(ToolDisplay::Grouped)
    }

    pub fn with_tool_display(tool_display: ToolDisplay) -> MessageBoxState {
        MessageBoxState {
            view: ConversationView::Welcome,
            viewport: MessageViewport::default(),
            transcript: MessageTranscript::new(tool_display),
            tool_history: ToolHistoryView::default(),
            scrollback: ScrollbackRenderer::new(),
            busy_indicator: BusyIndicator::default(),
            actor_state: State::Ready,
        }
    }

    pub fn append(&mut self, msg: Msg) {
        self.view = ConversationView::Transcript;
        self.transcript.append(msg, &self.formatter());
    }

    pub fn pop(&mut self) {
        self.transcript.pop_line();
    }

    pub fn clear(&mut self) {
        self.view = ConversationView::Welcome;
        self.transcript.clear();
        self.close_tool_history();
        self.scrollback.reset();
        self.busy_indicator.reset();
    }

    pub fn get_last(&self) -> Option<String> {
        self.transcript.last_line().cloned()
    }

    pub fn update_width_height(&mut self, width: u16, height: u16) {
        self.viewport.update(width, height);
        self.scrollback.update_width(self.viewport.wrap_width());
    }

    pub fn has_tool_history(&self) -> bool {
        self.transcript.has_tool_history()
    }

    pub fn tool_history_expanded(&self) -> bool {
        matches!(self.tool_history, ToolHistoryView::Expanded { .. })
    }

    pub fn close_tool_history(&mut self) {
        self.tool_history = ToolHistoryView::Collapsed;
    }

    pub fn handle_tool_history_key(&mut self, key: &KeyEvent) -> bool {
        match (self.tool_history, key.code) {
            (_, KeyCode::Char('o')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if key.kind == KeyEventKind::Press {
                    self.tool_history = match self.tool_history {
                        ToolHistoryView::Collapsed if self.has_tool_history() => {
                            ToolHistoryView::Expanded { offset: 0 }
                        }
                        _ => ToolHistoryView::Collapsed,
                    };
                }
                true
            }
            (ToolHistoryView::Collapsed, _) => false,
            (_, KeyCode::Esc | KeyCode::Char('q')) => {
                self.close_tool_history();
                true
            }
            (_, KeyCode::Char('c')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_tool_history();
                true
            }
            (ToolHistoryView::Expanded { offset }, code) => {
                let capacity = self.viewport.live_line_capacity(1);
                let max_offset = self
                    .transcript
                    .expanded_tool_rows(&self.formatter())
                    .len()
                    .saturating_sub(capacity);
                let offset = offset.min(max_offset);
                let offset = match code {
                    KeyCode::Up | KeyCode::Char('k') => offset.saturating_add(1),
                    KeyCode::Down | KeyCode::Char('j') => offset.saturating_sub(1),
                    KeyCode::PageUp => offset.saturating_add(capacity.max(1)),
                    KeyCode::PageDown => offset.saturating_sub(capacity.max(1)),
                    KeyCode::Home => max_offset,
                    KeyCode::End => 0,
                    _ => offset,
                };
                self.tool_history = ToolHistoryView::Expanded {
                    offset: offset.min(max_offset),
                };
                true
            }
        }
    }

    pub fn flush_scrollback<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> color_eyre::Result<()>
    where
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        let formatter = self.formatter();
        let flushed_lines = self
            .transcript
            .take_scrollback_rows(self.live_line_capacity(), &formatter);
        if !flushed_lines.is_empty() {
            let rendered_lines = self.scrollback.render_flushed_lines(&flushed_lines);
            terminal.insert_before(rendered_lines.len() as u16, |buf| {
                Paragraph::new(rendered_lines)
                    .style(theme::base())
                    .render(buf.area, buf);
            })?;
        }

        Ok(())
    }

    pub fn start_stream_message(&mut self, leading_blank_line: bool) {
        self.view = ConversationView::Transcript;
        self.transcript
            .start_stream(leading_blank_line, &self.formatter());
    }

    pub fn push_stream_message(&mut self, chunk: &str) {
        self.view = ConversationView::Transcript;
        self.transcript.push_stream_chunk(chunk);
    }

    pub fn finish_stream_message(&mut self, trailing_blank_line: bool) {
        self.transcript
            .finish_stream(trailing_blank_line, &self.formatter());
    }

    pub fn advance_busy_indicator(&mut self) {
        self.busy_indicator.advance(&self.actor_state);
    }

    pub fn animation_deadline(&self) -> Option<tokio::time::Instant> {
        match self.tool_history {
            ToolHistoryView::Collapsed => self.busy_indicator.deadline(&self.actor_state),
            ToolHistoryView::Expanded { .. } => None,
        }
    }

    pub fn render_history(&self, area: Rect, buf: &mut Buffer, offset: usize) {
        let lines = match self.tool_history {
            ToolHistoryView::Collapsed => {
                let lines = self.history_lines();
                let capacity = usize::from(area.height);
                let start = lines.len().saturating_sub(capacity).saturating_sub(offset);
                lines
                    .into_iter()
                    .skip(start)
                    .take(capacity)
                    .collect::<Vec<_>>()
            }
            ToolHistoryView::Expanded { .. } => self.output_lines(area.width),
        };
        Paragraph::new(lines).style(theme::base()).render(area, buf);
    }

    pub fn history_line_count(&self) -> usize {
        self.history_lines().len()
    }

    fn history_lines(&self) -> Vec<Line<'static>> {
        let formatter = self.formatter();
        let rows = self
            .transcript
            .committed_rows()
            .iter()
            .cloned()
            .chain(
                self.transcript
                    .active_rows(&formatter)
                    .into_iter()
                    .flatten(),
            )
            .collect::<Vec<_>>();
        self.scrollback
            .render_history_lines(&rows)
            .into_iter()
            .chain(self.busy_indicator.render_line(
                &self.actor_state,
                u16::try_from(self.viewport.wrap_width()).unwrap_or(u16::MAX),
            ))
            .collect()
    }

    fn formatter(&self) -> MessageFormatter {
        MessageFormatter::new(self.viewport.wrap_width())
    }

    pub fn clear_terminal(&mut self, terminal: &mut DefaultTerminal) -> color_eyre::Result<()> {
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

    fn output_lines(&self, width: u16) -> Vec<Line<'static>> {
        let formatter = self.formatter();
        match self.tool_history {
            ToolHistoryView::Collapsed => {
                let lines = self.scrollback.render_live_lines(
                    self.transcript.committed_rows(),
                    self.transcript.active_rows(&formatter),
                    self.busy_indicator.render_line(&self.actor_state, width),
                );
                self.viewport.visible_lines(lines)
            }
            ToolHistoryView::Expanded { offset } => {
                let rows = self.transcript.expanded_tool_rows(&formatter);
                let lines = self.scrollback.render_history_lines(&rows);
                let capacity = self.viewport.live_line_capacity(1);
                let start = lines.len().saturating_sub(capacity).saturating_sub(offset);
                std::iter::once(
                    Line::from(vec![
                        theme::badge("Tool history", theme::ACCENT),
                        theme::muted(" · Ctrl+o / Esc collapse"),
                    ])
                    .style(theme::base().bg(theme::SURFACE)),
                )
                .chain(lines.into_iter().skip(start).take(capacity))
                .collect()
            }
        }
    }

    fn live_line_capacity(&self) -> usize {
        self.viewport
            .live_line_capacity(self.busy_indicator.reserved_lines(&self.actor_state))
    }
}

impl Default for MessageBoxState {
    fn default() -> Self {
        Self::new()
    }
}

impl StatefulWidget for MessageBox {
    type State = MessageBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        match state.view {
            ConversationView::Welcome => Welcome.render(area, buf),
            ConversationView::Transcript => Paragraph::new(state.output_lines(area.width))
                .style(theme::base())
                .render(area, buf),
        }
    }
}
