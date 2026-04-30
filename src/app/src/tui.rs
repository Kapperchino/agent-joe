#![warn(clippy::pedantic)]

use actors::actor::Message;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::State;
use common_models::tui_models::TokenCount;
use std::str::FromStr;
use std::time::Duration;

use crate::widgets::input_box::{InputBox, InputBoxState};
use crate::widgets::message_box::{MessageBox, MessageBoxState, Msg};
use clients::config::ConfigContext;
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
    do_clear_terminal: bool,
    config_context: ConfigContext,
}
#[derive(Clone, Copy)]
pub enum InputMode {
    None,
    HomeMenu(HomeMenu),
    CommandMenu(CommandMenu),
}

impl Default for InputMode {
    fn default() -> Self {
        Self::HomeMenu(HomeMenu::Normal)
    }
}

#[derive(Default, Clone, Copy)]
pub enum HomeMenu {
    #[default]
    Normal,
    InputCommand,
    Editing,
}

#[derive(Default, Clone, Copy)]
pub enum CommandMenu {
    #[default]
    ModelSelector,
}

impl TUIApp {
    pub fn new(
        actor_ref: ActorRef<Message>,
        config_context: ConfigContext,
        debug_mode: bool,
    ) -> Self {
        let config = config_context.get_config();
        Self {
            input_mode: Default::default(),
            do_quit: false,
            actor_ref: actor_ref.clone(),
            actor_state: State::Ready,
            token_count: TokenCount::default(),
            debug_mode,
            input_box: InputBoxState::new(config),
            message_box: MessageBoxState::new(),
            do_clear_terminal: false,
            config_context,
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
                    match command {
                        Command::PrintContext => {
                            match self.actor_ref.send_message(Message::Command(command)) {
                                Ok(_) => {}
                                Err(_) => {
                                    eprintln!("it's joever")
                                }
                            };
                            self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                        }
                        Command::Logout => {
                            match self.actor_ref.send_message(Message::Command(command)) {
                                Ok(_) => {}
                                Err(_) => {
                                    eprintln!("it's joever")
                                }
                            };
                            self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                        }
                        Command::Clear => {
                            self.clear_messages_and_terminal();
                            match self.actor_ref.send_message(Message::Command(command)) {
                                Ok(_) => {}
                                Err(_) => {
                                    eprintln!("it's joever")
                                }
                            };
                            self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                        }
                        Command::ChangeModel(_, _) => self
                            .update_input_mode(InputMode::CommandMenu(CommandMenu::ModelSelector)),
                    }
                    //self.message_box.append(Msg::Message(submitted_command));
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
        self.input_mode = mode;
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

            self.message_box
                .flush_scrollback(&mut terminal, self.do_clear_terminal)?;
            terminal.draw(|frame| self.draw(frame))?;

            if self.do_clear_terminal {
                self.do_clear_terminal = false;
            }
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
            ActorToTui::CommandResult(command, command_res) => {
                self.message_box.append(Msg::Message(command_res));
                match command {
                    Command::Logout => self.kill(),
                    Command::Clear => self.clear_messages_and_terminal(),
                    Command::PrintContext => {}
                    Command::ChangeModel(_, _) => {}
                }
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
                InputMode::HomeMenu(HomeMenu::Editing | HomeMenu::InputCommand) => {
                    self.input_box.paste(text);
                }
                InputMode::HomeMenu(HomeMenu::Normal)
                | InputMode::CommandMenu(CommandMenu::ModelSelector)
                | InputMode::None => {}
            },
            Event::Resize(_, _) => {}
        }
    }

    fn handle_key_event(&mut self, key: &KeyEvent) {
        match self.input_mode {
            InputMode::HomeMenu(HomeMenu::Normal) | InputMode::None => match key.code {
                KeyCode::Char('i') => {
                    self.update_input_mode(InputMode::HomeMenu(HomeMenu::Editing));
                }
                KeyCode::Char('/') => {
                    self.update_input_mode(InputMode::HomeMenu(HomeMenu::InputCommand));
                }
                KeyCode::Char('q') => self.kill(),
                KeyCode::Char('c') => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        if let State::Ready = self.actor_state {
                            self.kill()
                        } else {
                            self.interrupt();
                        }
                    }
                }
                _ => {}
            },
            InputMode::HomeMenu(HomeMenu::Editing) => match key.code {
                KeyCode::Enter => {
                    if key.modifiers.is_empty() {
                        self.submit_message();
                        self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                    } else if key.modifiers.contains(KeyModifiers::ALT)
                        || key.modifiers.contains(KeyModifiers::SHIFT)
                    {
                        self.input_box.enter_char('\n');
                    }
                }
                KeyCode::Char(char) => match char {
                    'n' => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            self.input_box.enter_char('\n');
                        } else {
                            self.input_box.enter_char(char)
                        }
                    }
                    'c' => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            self.input_box.clear();
                            self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                        } else {
                            self.input_box.enter_char(char)
                        }
                    }
                    _ => self.input_box.enter_char(char),
                },

                KeyCode::Backspace => {
                    if self.input_box.is_empty() {
                        self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                    } else {
                        self.input_box.delete_char()
                    }
                }
                KeyCode::Left => self.input_box.move_cursor_left(),
                KeyCode::Right => self.input_box.move_cursor_right(),
                KeyCode::Esc => self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal)),
                _ => {}
            },
            InputMode::HomeMenu(HomeMenu::InputCommand) => match key.code {
                KeyCode::Enter => {
                    self.submit_command();
                }
                KeyCode::Tab => {
                    self.input_box.auto_comp_command();
                }
                KeyCode::Char(to_insert) => match to_insert {
                    'c' => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            self.input_box.clear();
                            self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                        } else {
                            self.input_box.enter_char(to_insert)
                        }
                    }
                    _ => self.input_box.enter_char(to_insert),
                },
                KeyCode::Backspace => {
                    if self.input_box.is_empty() {
                        self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal));
                    } else {
                        self.input_box.delete_char()
                    }
                }
                KeyCode::Left => self.input_box.move_cursor_left(),
                KeyCode::Right => self.input_box.move_cursor_right(),
                KeyCode::Esc => self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal)),
                _ => {}
            },
            InputMode::CommandMenu(menu) => match menu {
                CommandMenu::ModelSelector => match key.code {
                    KeyCode::Down | KeyCode::Char('j') => self.input_box.select_next_model(),
                    KeyCode::Up | KeyCode::Char('k') => self.input_box.select_previous_model(),
                    KeyCode::Esc => self.update_input_mode(InputMode::HomeMenu(HomeMenu::Normal)),
                    KeyCode::Enter => self.input_box.confirm_select_model(),
                    _ => {}
                },
            },
        }
    }

    fn kill(&mut self) {
        self.do_quit = true;
        self.actor_ref.kill();
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

    fn clear_messages_and_terminal(&mut self) {
        self.message_box.clear();
        self.do_clear_terminal = true;
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

        frame.render_stateful_widget(InputBox::new(), input_area, &mut self.input_box);

        let cursor_pos = self.input_box.get_cursor_pos(&input_area);

        match self.input_mode {
            // Hide the cursor. `Frame` does this by default, so we don't need to do anything here
            InputMode::HomeMenu(HomeMenu::Normal) | InputMode::None => {}

            InputMode::HomeMenu(HomeMenu::InputCommand) => frame.set_cursor_position(cursor_pos),

            // Make the cursor visible and ask ratatui to put it at the specified coordinates after
            // rendering
            #[allow(clippy::cast_possible_truncation)]
            InputMode::HomeMenu(HomeMenu::Editing) => frame.set_cursor_position(cursor_pos),
            InputMode::CommandMenu(_) => {}
        }

        frame.render_stateful_widget(MessageBox {}, msg_area, &mut self.message_box);
    }
}
