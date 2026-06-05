use crate::widgets::message_box::format::MessageFormatter;
use crate::widgets::message_box::indicator::BusyIndicator;
use crate::widgets::message_box::scrollback::ScrollbackRenderer;
use crate::widgets::message_box::transcript::MessageTranscript;
use crate::widgets::message_box::viewport::MessageViewport;
use common_models::tui_models::State;
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use ratatui::DefaultTerminal;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, StatefulWidget};
use ratatui::widgets::{Paragraph, Widget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Message(String),
    Tool(String),
    Empty,
}

pub struct MessageBox {}

pub struct MessageBoxState {
    viewport: MessageViewport,
    transcript: MessageTranscript,
    scrollback: ScrollbackRenderer,
    busy_indicator: BusyIndicator,
    pub actor_state: State,
}

impl MessageBoxState {
    pub fn new() -> MessageBoxState {
        MessageBoxState {
            viewport: MessageViewport::default(),
            transcript: MessageTranscript::default(),
            scrollback: ScrollbackRenderer::new(),
            busy_indicator: BusyIndicator::default(),
            actor_state: State::Ready,
        }
    }

    pub fn append(&mut self, msg: Msg) {
        self.transcript.append(msg, &self.formatter());
    }

    pub fn pop(&mut self) {
        self.transcript.pop_line();
    }

    pub fn clear(&mut self) {
        self.transcript.clear();
        self.scrollback.reset();
        self.busy_indicator.reset();
    }

    pub fn get_last(&self) -> Option<String> {
        self.transcript.last_line().cloned()
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
        let flushed_lines = self
            .transcript
            .take_scrollback_overflow(self.live_line_capacity(), &formatter);
        if flushed_lines.is_empty() {
            return Ok(());
        }

        let rendered_lines = self.scrollback.render_flushed_lines(&flushed_lines);
        terminal.insert_before(rendered_lines.len() as u16, |buf| {
            Paragraph::new(rendered_lines).render(buf.area, buf);
        })?;

        Ok(())
    }

    pub fn start_stream_message(&mut self, leading_blank_line: bool) {
        self.transcript
            .start_stream(leading_blank_line, &self.formatter());
    }

    pub fn push_stream_message(&mut self, chunk: &str) {
        self.transcript.push_stream_chunk(chunk);
    }

    pub fn finish_stream_message(&mut self, trailing_blank_line: bool) {
        self.transcript
            .finish_stream(trailing_blank_line, &self.formatter());
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
            self.transcript.committed_lines(),
            self.transcript.active_lines(&formatter),
            self.busy_indicator.render_line(&self.actor_state),
        );
        self.viewport.visible_lines(lines)
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
        Paragraph::new(state.output_lines()).render(area, buf);
    }
}
