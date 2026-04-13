#![warn(clippy::pedantic)]

use actors::actor::Message;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::Command;
use common_models::tui_models::State;
use common_models::tui_models::TokenCount;
use std::time::Duration;

use crate::draw_line::{DrawLine, RenderState};
use crate::input_box::{InputBox, InputBoxState};
use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ractor::ActorRef;
use ratatui::layout::Position;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{Event, KeyCode},
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use throbber_widgets_tui::{Throbber, ThrobberState};
use tokio::sync::mpsc::UnboundedReceiver;

pub struct TUIApp {
    messages: Vec<String>,
    input_mode: InputMode,
    do_quit: bool,
    msg_area_height: usize,
    msg_area_width: usize,
    actor_ref: ActorRef<Message>,
    actor_state: State,
    throbber_state: ThrobberState,
    throbber_tick: usize,
    token_count: TokenCount,
    debug_mode: bool,
    draw_line: DrawLine,
    scrollback_render_state: RenderState,
    input_box: InputBoxState,
}
#[derive(Default, Clone)]
pub enum InputMode {
    #[default]
    Normal,
    InputCommand,
    Editing,
}

impl TUIApp {
    pub fn new(actor_ref: ActorRef<Message>, debug_mode: bool) -> Self {
        Self {
            messages: vec![],
            input_mode: Default::default(),
            do_quit: false,
            msg_area_height: 0,
            msg_area_width: 0,
            actor_ref: actor_ref.clone(),
            actor_state: State::Ready,
            throbber_state: ThrobberState::default(),
            throbber_tick: 0,
            token_count: TokenCount::default(),
            debug_mode,
            draw_line: DrawLine::new(),
            scrollback_render_state: RenderState::default(),
            input_box: InputBoxState::new(),
        }
    }

    fn max_live_messages(&self) -> usize {
        let throbber_reserved = usize::from(matches!(self.actor_state, State::ThinkingStart));
        self.msg_area_height
            .saturating_sub(throbber_reserved)
            .max(1)
    }

    fn submit_message(&mut self) {
        if !self.input_box.is_empty() {
            match self
                .actor_ref
                .send_message(Message::StartWork(Some(self.input_box.get_input())))
            {
                Ok(_) => {}
                Err(_) => {
                    eprintln!("it's joever")
                }
            };

            self.messages
                .append(&mut self.wrap_str(&self.input_box.get_input()));
            self.input_box.clear();
        }
    }

    fn submit_command(&mut self) {
        if !self.input_box.is_empty() {
            let input = self.input_box.get_input();
            let submitted_command = format!("/{}", &input);
            let command = Command::parse(&input);
            match command {
                Ok(command) => {
                    match self.actor_ref.send_message(Message::Command(command)) {
                        Ok(_) => {}
                        Err(_) => {
                            eprintln!("it's joever")
                        }
                    };

                    self.messages.append(&mut self.wrap_str(&submitted_command));
                }
                Err(err) => {
                    self.messages.append(&mut self.wrap_str(&submitted_command));
                    self.messages.append(&mut self.wrap_str(&err.to_string()));
                }
            }

            self.input_box.clear();
        }
    }

    fn update_input_mode(&mut self, mode: InputMode) {
        self.input_mode = mode.clone();
        self.input_box.input_mode = mode;
    }

    fn wrap_str(&self, string: &str) -> Vec<String> {
        let wrap_width = self.msg_area_width.saturating_sub(2).max(1);
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
                textwrap::Options::new(wrap_width)
                    .initial_indent("- ")
                    .subsequent_indent("  "),
            )
            .into_iter()
            .map(|x| x.to_string())
            .collect();
        }

        self.wrap_str(string)
    }

    fn flush_scrollback(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
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

    fn advance_throbber(&mut self) {
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

    pub async fn run(
        mut self,
        mut terminal: DefaultTerminal,
        mut actor_rx: UnboundedReceiver<ActorToTui>,
    ) -> Result<()> {
        let mut events = EventStream::new();

        let period = Duration::from_secs_f32(1.0 / 120.0);
        let mut interval = tokio::time::interval(period);

        while !self.do_quit {
            tokio::select! {
                _ = interval.tick() => self.advance_throbber(),
                Some(Ok(event)) = events.next() => self.handle_term_event(&event),
                Some(actor_msg) = actor_rx.recv() => self.handle_actor_msg(actor_msg),
            }

            self.flush_scrollback(&mut terminal)?;
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn handle_actor_msg(&mut self, msg: ActorToTui) {
        match msg {
            ActorToTui::StateChanged(state) => {
                self.actor_state = state;
                match self.actor_state {
                    State::MessageStart => self.messages.push(String::new()),
                    State::MessageStop => self.messages.push(String::new()),
                    _ => {}
                }
            }
            ActorToTui::Data(data) => match self.actor_state {
                State::Ready => {}
                State::StreamStart => {}
                State::StreamStop => {}
                State::ThinkingStart => {
                    let last = self.messages.last_mut().cloned();
                    match last {
                        None => {
                            let mut wrapped = self.wrap_str(&data);
                            self.messages.append(&mut wrapped);
                        }
                        Some(mut buff) => {
                            buff.push_str(data.as_str());
                            let mut wrapped = self.wrap_str(&buff);
                            self.messages.pop();
                            self.messages.append(&mut wrapped)
                        }
                    }
                }
                State::ThinkingStop => {}
                State::MessageStart => {
                    let last = self.messages.last_mut().cloned();
                    match last {
                        None => {
                            let mut wrapped = self.wrap_str(&data);
                            self.messages.append(&mut wrapped);
                        }
                        Some(mut buff) => {
                            buff.push_str(data.as_str());
                            let mut wrapped = self.wrap_str(&buff);
                            self.messages.pop();
                            self.messages.append(&mut wrapped)
                        }
                    }
                }
                State::MessageStop => {}
                State::ToolStart => {}
                State::ToolStop => {}
                State::Stopped => {}
            },
            ActorToTui::ToolUse(lines) => {
                lines.into_iter().for_each(|line| {
                    self.messages.append(&mut self.wrap_tool_str(&line));
                });
            }
            ActorToTui::CommandResult(_, command_res) => {
                let mut wrapped = self.wrap_str(&command_res);
                self.messages.append(&mut wrapped);
            }
            ActorToTui::TokensUpdated(token_count) => {
                self.token_count = token_count;
            }
        }
    }

    fn handle_term_event(&mut self, event: &Event) {
        match event {
            Event::FocusGained => {}
            Event::FocusLost => {}
            Event::Key(key) => self.handle_key_event(key),
            Event::Mouse(_) => {}
            Event::Paste(text) => match self.input_mode {
                InputMode::Editing | InputMode::InputCommand => self.input_box.paste(text),
                InputMode::Normal => {}
            },
            Event::Resize(_, _) => {}
        }
    }

    fn handle_key_event(&mut self, key: &KeyEvent) {
        match self.input_mode {
            InputMode::Normal => match key.code {
                KeyCode::Char('i') => self.update_input_mode(InputMode::Editing),
                KeyCode::Char('/') => self.update_input_mode(InputMode::InputCommand),
                KeyCode::Char('q') => {
                    self.do_quit = true;
                    self.actor_ref.kill();
                }
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => {
                    if key.modifiers.is_empty() {
                        self.submit_message();
                        self.update_input_mode(InputMode::Normal);
                    } else if key.modifiers.contains(KeyModifiers::ALT)
                        || key.modifiers.contains(KeyModifiers::SHIFT)
                    {
                        self.input_box.enter_char('\n');
                    }
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input_box.enter_char('\n');
                }
                KeyCode::Char(to_insert) => self.input_box.enter_char(to_insert),
                KeyCode::Backspace => self.input_box.delete_char(),
                KeyCode::Left => self.input_box.move_cursor_left(),
                KeyCode::Right => self.input_box.move_cursor_right(),
                KeyCode::Esc => self.update_input_mode(InputMode::Normal),
                _ => {}
            },
            InputMode::InputCommand => match key.code {
                KeyCode::Enter => {
                    self.submit_command();
                    self.update_input_mode(InputMode::Normal);
                }
                KeyCode::Char(to_insert) => self.input_box.enter_char(to_insert),
                KeyCode::Backspace => self.input_box.delete_char(),
                KeyCode::Left => self.input_box.move_cursor_left(),
                KeyCode::Right => self.input_box.move_cursor_right(),
                KeyCode::Esc => self.update_input_mode(InputMode::Normal),
                _ => {}
            },
        }
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn draw(&mut self, frame: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(self.input_box.get_height(frame.area().width)),
            Constraint::Length(1),
        ]);

        let [msg_area, input_area, token_area] = chunks.areas(frame.area());

        self.msg_area_height = msg_area.height as usize;
        self.msg_area_width = msg_area.width as usize;

        // ── token counter: pretty spans, bottom-right of input box ─────────
        let token_line = Line::from(vec![
            Span::styled(
                " ↑ ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.token_count.input_tokens.to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  ↓ ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.token_count.output_tokens.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]);

        let token_block = Block::new().title_bottom(token_line);
        frame.render_widget(token_block, token_area);

        frame.render_stateful_widget(InputBox {}, input_area, &mut self.input_box);

        let cursor_pos = self.input_box.get_cursor_pos(&input_area);

        match self.input_mode {
            // Hide the cursor. `Frame` does this by default, so we don't need to do anything here
            InputMode::Normal => {}

            InputMode::InputCommand => frame.set_cursor_position(cursor_pos),

            // Make the cursor visible and ask ratatui to put it at the specified coordinates after
            // rendering
            #[allow(clippy::cast_possible_truncation)]
            InputMode::Editing => frame.set_cursor_position(cursor_pos),
        }

        let messages = Paragraph::new(self.output_lines());
        frame.render_widget(messages, msg_area);
    }
}
