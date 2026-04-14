use crate::draw_line::{DrawLine, RenderState};
use crate::input_box::{InputBox, InputBoxState};
use crate::tui::InputMode;
use common_models::tui_models::State;
use ratatui::DefaultTerminal;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, StatefulWidget, Style};
use ratatui::widgets::{Paragraph, Widget};
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
        string
            .split('\n')
            .flat_map(|line| {
                if line.len() <= wrap_width {
                    vec![line.to_string()]
                } else {
                    textwrap::wrap(line, textwrap::Options::new(wrap_width))
                        .into_iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                }
            })
            .collect()
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
        lines
    }

    fn max_live_messages(&self) -> usize {
        let throbber_reserved = usize::from(matches!(self.actor_state, State::ThinkingStart));
        (self.msg_area_height as usize)
            .saturating_sub(throbber_reserved)
            .max(1)
    }

    pub(crate) fn flush_scrollback(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> color_eyre::Result<()> {
        let max_live_messages = self.max_live_messages();
        if self.messages.len() <= max_live_messages {
            return Ok(());
        }

        let flush_count = self.messages.len().saturating_sub(max_live_messages);
        let flushed_lines = self.messages.drain(0..flush_count).collect::<Vec<_>>();
        let rendered_lines = self
            .draw_line
            .render_lines_with_state(&flushed_lines, &mut self.scrollback_render_state);

        terminal.insert_before(rendered_lines.len() as u16, |buf| {
            Paragraph::new(rendered_lines).render(buf.area, buf);
        })?;

        Ok(())
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
