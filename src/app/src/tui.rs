#![warn(clippy::pedantic)]

use actors::actor::Message;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::State;
use common_models::tui_models::TokenCount;
use std::str::FromStr;
use std::time::Duration;

use crate::input_box::{InputBox, InputBoxState};
use crate::message_box::{MessageBox, MessageBoxState, Msg};
use color_eyre::Result;
use commands::command::Command;
use crossterm::event::{EventStream, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ractor::ActorRef;
use ratatui::{
    crossterm::event::{Event, KeyCode}, layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Block,
    DefaultTerminal,
    Frame,
};
use tokio::sync::mpsc::UnboundedReceiver;

pub struct TUIApp {
    input_mode: InputMode,
    do_quit: bool,
    actor_ref: ActorRef<Message>,
    actor_state: State,
    token_count: TokenCount,
    debug_mode: bool,
    input_box: InputBoxState,
    message_box: MessageBoxState,
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
            input_mode: Default::default(),
            do_quit: false,
            actor_ref: actor_ref.clone(),
            actor_state: State::Ready,
            token_count: TokenCount::default(),
            debug_mode,
            input_box: InputBoxState::new(),
            message_box: MessageBoxState::new(),
        }
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

            self.message_box
                .append(Msg::Message(self.input_box.get_input()));
            self.input_box.clear();
        }
    }

    fn submit_command(&mut self) {
        if !self.input_box.is_empty() {
            let input = self.input_box.get_input();
            let submitted_command = format!("/{}", &input);
            let command = Command::from_str(&input);
            match command {
                Ok(command) => {
                    match self.actor_ref.send_message(Message::Command(command)) {
                        Ok(_) => {}
                        Err(_) => {
                            eprintln!("it's joever")
                        }
                    };

                    self.message_box.append(Msg::Message(submitted_command));
                }
                Err(err) => {
                    self.message_box.append(Msg::Message(submitted_command));
                    self.message_box.append(Msg::Message(err.to_string()));
                }
            }

            self.input_box.clear();
        }
    }

    fn update_input_mode(&mut self, mode: InputMode) {
        self.input_mode = mode.clone();
        self.input_box.input_mode = mode;
    }

    fn update_actor_state(&mut self, state: State) {
        self.actor_state = state.clone();
        self.message_box.actor_state = state;
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
                _ = interval.tick() => self.message_box.advance_throbber(),
                Some(Ok(event)) = events.next() => self.handle_term_event(&event),
                Some(actor_msg) = actor_rx.recv() => self.handle_actor_msg(actor_msg),
            }

            self.message_box.flush_scrollback(&mut terminal)?;
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn handle_actor_msg(&mut self, msg: ActorToTui) {
        match msg {
            ActorToTui::StateChanged(state) => {
                self.update_actor_state(state);
                match self.actor_state {
                    State::ThinkingStart => self.message_box.start_stream_message(false),
                    State::ThinkingStop => self.message_box.finish_stream_message(false),
                    State::MessageStart => self.message_box.start_stream_message(true),
                    State::MessageStop => self.message_box.finish_stream_message(true),
                    _ => {}
                }
            }
            ActorToTui::Data(data) => match self.actor_state {
                State::Ready => {}
                State::StreamStart => {}
                State::StreamStop => {}
                State::ThinkingStart => self.message_box.push_stream_message(&data),
                State::ThinkingStop => {}
                State::MessageStart => self.message_box.push_stream_message(&data),
                State::MessageStop => {}
                State::ToolStart => {}
                State::ToolStop => {}
                State::Stopped => {}
            },
            ActorToTui::ToolUse(lines) => {
                lines.into_iter().for_each(|line| {
                    self.message_box.append(Msg::Tool(line));
                });
            }
            ActorToTui::CommandResult(_, command_res) => {
                self.message_box.append(Msg::Message(command_res))
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
                KeyCode::Char('c') => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        if let State::Ready = self.actor_state {
                            self.do_quit = true;
                            self.actor_ref.kill();
                        } else {
                            self.interrupt();
                        }
                    }
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
                KeyCode::Char('c') => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        self.input_box.clear();
                        self.update_input_mode(InputMode::Normal);
                    }
                }
                KeyCode::Char(to_insert) => self.input_box.enter_char(to_insert),
                KeyCode::Backspace => {
                    if self.input_box.is_empty() {
                        self.update_input_mode(InputMode::Normal)
                    } else {
                        self.input_box.delete_char()
                    }
                }
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
                KeyCode::Tab => {
                    self.input_box.auto_comp_command();
                }
                KeyCode::Char(to_insert) => match to_insert {
                    'c' => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            self.input_box.clear();
                            self.update_input_mode(InputMode::Normal);
                        } else {
                            self.input_box.enter_char(to_insert)
                        }
                    }
                    _ => self.input_box.enter_char(to_insert),
                },
                KeyCode::Backspace => {
                    if self.input_box.is_empty() {
                        self.update_input_mode(InputMode::Normal)
                    } else {
                        self.input_box.delete_char()
                    }
                }
                KeyCode::Left => self.input_box.move_cursor_left(),
                KeyCode::Right => self.input_box.move_cursor_right(),
                KeyCode::Esc => self.update_input_mode(InputMode::Normal),
                _ => {}
            },
        }
    }

    fn interrupt(&mut self) {
        match self.actor_ref.send_message(Message::Interrupt) {
            Ok(_) => {}
            Err(_) => {
                eprintln!("it's joever")
            }
        };
        self.message_box
            .append(Msg::Message("Interrupted".to_string()))
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn draw(&mut self, frame: &mut Frame) {
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(self.input_box.get_height(frame.area().width)),
            Constraint::Length(1),
        ]);

        let [msg_area, input_area, token_area] = chunks.areas(frame.area());

        self.message_box
            .update_width_height(msg_area.width, msg_area.height);

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

        frame.render_stateful_widget(MessageBox {}, msg_area, &mut self.message_box);
    }
}
