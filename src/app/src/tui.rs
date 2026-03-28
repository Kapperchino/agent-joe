#![warn(clippy::pedantic)]

use actors::actor::Message;
use common_models::tui_models::ActorToTui;
use common_models::tui_models::Command;
use common_models::tui_models::State;
use common_models::tui_models::TokenCount;
use std::time::Duration;

use crate::draw_line::DrawLine;
use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent};
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
    character_index: usize,
    input: String,
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
}
#[derive(Default)]
enum InputMode {
    #[default]
    Normal,
    InputCommand,
    Editing,
}

impl TUIApp {
    const COMMAND_PROMPT: &str = "/ ";
    const COMMAND_CONTINUATION: &str = "  ";

    pub fn new(actor_ref: ActorRef<Message>) -> Self {
        Self {
            character_index: 0,
            input: String::new(),
            messages: vec![],
            input_mode: Default::default(),
            do_quit: false,
            msg_area_height: 0,
            msg_area_width: 0,
            actor_ref,
            actor_state: State::Ready,
            throbber_state: ThrobberState::default(),
            throbber_tick: 0,
            token_count: TokenCount::default(),
        }
    }

    fn max_live_messages(&self) -> usize {
        let throbber_reserved = usize::from(matches!(self.actor_state, State::ThinkingStart));
        self.msg_area_height
            .saturating_sub(throbber_reserved)
            .max(1)
    }

    fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }

    fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
    }

    fn paste(&mut self, string: &String) {
        self.input.push_str(string);
        let cursor_moved_right = self.character_index.saturating_add(string.chars().count());
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
    }

    fn delete_char(&mut self) {
        let is_not_cursor_leftmost = self.character_index != 0;
        if is_not_cursor_leftmost {
            // Method "remove" is not used on the saved text for deleting the selected char.
            // Reason: Using remove on String works on bytes instead of the chars.
            // Using remove would require special care because of char boundaries.

            let current_index = self.character_index;
            let from_left_to_current_index = current_index - 1;

            // Getting all characters before the selected character.
            let before_char_to_delete = self.input.chars().take(from_left_to_current_index);
            // Getting all characters after selected character.
            let after_char_to_delete = self.input.chars().skip(current_index);

            // Put all characters together except the selected one.
            // By leaving the selected one out, it is forgotten and therefore deleted.
            self.input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    fn submit_message(&mut self) {
        if !self.input.is_empty() {
            match self
                .actor_ref
                .send_message(Message::StartWork(Some(self.input.to_string())))
            {
                Ok(_) => {}
                Err(_) => {
                    eprintln!("it's joever")
                }
            };

            self.messages.append(&mut self.wrap_str(&self.input));
            self.input.clear();
            self.reset_cursor();
        }
    }

    fn submit_command(&mut self) {
        if !self.input.is_empty() {
            let submitted_command = format!("/{}", self.input);
            let command = Command::parse(self.input.as_str());
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

            self.input.clear();
            self.reset_cursor();
        }
    }

    fn wrap_str(&self, string: &str) -> Vec<String> {
        let wrap_width = self.msg_area_width.saturating_sub(2).max(1);
        textwrap::wrap(string, textwrap::Options::new(wrap_width))
            .into_iter()
            .map(|x| x.to_string())
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
        let rendered_lines = DrawLine::render_lines(&flushed_lines);

        terminal.insert_before(flush_count as u16, |buf| {
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
        let mut lines = DrawLine::render_lines(&self.messages);
        if matches!(self.actor_state, State::ThinkingStart) {
            lines.push(Self::thinking_throbber().to_line(&self.throbber_state));
        }
        lines
    }

    fn input_wrap_width(total_width: u16) -> usize {
        usize::from(total_width.saturating_sub(2).max(1))
    }

    fn wrap_input_text(
        text: &str,
        wrap_width: usize,
        initial_indent: &str,
        subsequent_indent: &str,
    ) -> Vec<String> {
        let wrap_width = wrap_width.max(1);
        let initial_indent_width = textwrap::core::display_width(initial_indent);
        let subsequent_indent_width = textwrap::core::display_width(subsequent_indent);

        let mut lines = Vec::new();
        let mut current_line = initial_indent.to_string();
        let mut current_width = initial_indent_width;

        for ch in text.chars() {
            if ch == '\n' {
                lines.push(current_line);
                current_line = subsequent_indent.to_string();
                current_width = subsequent_indent_width;
                continue;
            }

            let ch_width = textwrap::core::display_width(ch.encode_utf8(&mut [0; 4]));
            if current_width + ch_width > wrap_width {
                lines.push(current_line);
                current_line = subsequent_indent.to_string();
                current_width = subsequent_indent_width;
            }

            current_line.push(ch);
            current_width += ch_width;
        }

        lines.push(current_line);
        lines
    }

    fn cursor_wrap_position(
        &self,
        wrap_width: usize,
        initial_indent: &str,
        subsequent_indent: &str,
    ) -> (u16, u16) {
        let cursor_prefix: String = self.input.chars().take(self.character_index).collect();
        let wrapped_prefix = Self::wrap_input_text(
            &cursor_prefix,
            wrap_width,
            initial_indent,
            subsequent_indent,
        );
        let mut cursor_y = wrapped_prefix.len().saturating_sub(1);
        let mut cursor_x = wrapped_prefix
            .last()
            .map(|line| textwrap::core::display_width(line))
            .unwrap_or_default();

        if cursor_x >= wrap_width {
            cursor_y = cursor_y.saturating_add(1);
            cursor_x = textwrap::core::display_width(subsequent_indent);
        }

        (cursor_x as u16, cursor_y as u16)
    }

    fn input_lines(&self, wrap_width: usize, min_lines: usize) -> Vec<Line<'static>> {
        let mut lines = Self::wrap_input_text(&self.input, wrap_width, "", "");
        lines.resize_with(min_lines.max(1), String::new);
        lines.into_iter().map(Line::from).collect()
    }

    fn command_lines(&self, wrap_width: usize, min_input_lines: usize) -> Vec<Line<'static>> {
        let mut command_input = Self::wrap_input_text(
            &self.input,
            wrap_width,
            Self::COMMAND_PROMPT,
            Self::COMMAND_CONTINUATION,
        );
        command_input.resize_with(min_input_lines.max(1), || {
            Self::COMMAND_CONTINUATION.to_string()
        });

        let mut lines = vec![Line::from(vec![
            Span::styled(
                Self::COMMAND_PROMPT,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                command_input
                    .remove(0)
                    .strip_prefix(Self::COMMAND_PROMPT)
                    .unwrap_or_default()
                    .to_string(),
                Style::default().fg(Color::Green),
            ),
        ])];

        lines.extend(
            command_input
                .into_iter()
                .map(|line| Line::from(Span::styled(line, Style::default().fg(Color::Green)))),
        );

        lines.extend([
            Line::from(Span::styled(
                "Enter to run, Esc to cancel",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Available commands",
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(
                    "context",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled("Print current context", Style::default().fg(Color::Gray)),
            ]),
        ]);

        lines
    }

    fn thinking_throbber() -> Throbber<'static> {
        Throbber::default()
            .label("thinking")
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
                State::ThinkingStart => {}
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
                InputMode::Editing | InputMode::InputCommand => self.paste(text),
                InputMode::Normal => {}
            },
            Event::Resize(_, _) => {}
        }
    }

    fn handle_key_event(&mut self, key: &KeyEvent) {
        match self.input_mode {
            InputMode::Normal => match key.code {
                KeyCode::Char('i') => {
                    self.input_mode = InputMode::Editing;
                }
                KeyCode::Char('/') => self.input_mode = InputMode::InputCommand,
                KeyCode::Char('q') => {
                    self.do_quit = true;
                    self.actor_ref.kill();
                }
                _ => {}
            },
            InputMode::Editing => match key.code {
                KeyCode::Enter => {
                    self.submit_message();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char(to_insert) => self.enter_char(to_insert),
                KeyCode::Backspace => self.delete_char(),
                KeyCode::Left => self.move_cursor_left(),
                KeyCode::Right => self.move_cursor_right(),
                KeyCode::Esc => self.input_mode = InputMode::Normal,
                _ => {}
            },
            InputMode::InputCommand => match key.code {
                KeyCode::Enter => {
                    self.submit_command();
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char(to_insert) => self.enter_char(to_insert),
                KeyCode::Backspace => self.delete_char(),
                KeyCode::Left => self.move_cursor_left(),
                KeyCode::Right => self.move_cursor_right(),
                KeyCode::Esc => self.input_mode = InputMode::Normal,
                _ => {}
            },
        }
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn draw(&mut self, frame: &mut Frame) {
        let input_wrap_width = Self::input_wrap_width(frame.area().width);
        let editing_display_lines =
            Self::wrap_input_text(&self.input, input_wrap_width, "", "").len();
        let command_display_lines = Self::wrap_input_text(
            &self.input,
            input_wrap_width,
            Self::COMMAND_PROMPT,
            Self::COMMAND_CONTINUATION,
        )
        .len();
        let editing_cursor = self.cursor_wrap_position(input_wrap_width, "", "");
        let command_cursor = self.cursor_wrap_position(
            input_wrap_width,
            Self::COMMAND_PROMPT,
            Self::COMMAND_CONTINUATION,
        );
        let editing_input_lines = editing_display_lines.max(usize::from(editing_cursor.1) + 1);
        let command_input_lines = command_display_lines.max(usize::from(command_cursor.1) + 1);
        let input_height = if matches!(self.input_mode, InputMode::InputCommand) {
            command_input_lines as u16 + 5
        } else {
            editing_input_lines as u16 + 2
        };
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(input_height),
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

        match self.input_mode {
            InputMode::InputCommand => {
                let command_section =
                    Paragraph::new(self.command_lines(input_wrap_width, command_input_lines))
                        .block(Block::bordered().title("Command"));
                frame.render_widget(command_section, input_area);
            }
            _ => {
                let input = Paragraph::new(self.input_lines(input_wrap_width, editing_input_lines))
                    .style(match self.input_mode {
                        InputMode::Normal => Style::default(),
                        InputMode::Editing => Style::default().fg(Color::Yellow),
                        InputMode::InputCommand => Style::default().fg(Color::Green),
                    })
                    .block(Block::bordered().title("Input"));
                frame.render_widget(input, input_area);
            }
        }

        match self.input_mode {
            // Hide the cursor. `Frame` does this by default, so we don't need to do anything here
            InputMode::Normal => {}

            InputMode::InputCommand => frame.set_cursor_position(Position::new(
                input_area.x + command_cursor.0 + 1,
                input_area.y + command_cursor.1 + 1,
            )),

            // Make the cursor visible and ask ratatui to put it at the specified coordinates after
            // rendering
            #[allow(clippy::cast_possible_truncation)]
            InputMode::Editing => frame.set_cursor_position(Position::new(
                input_area.x + editing_cursor.0 + 1,
                input_area.y + editing_cursor.1 + 1,
            )),
        }

        let messages = Paragraph::new(self.output_lines());
        frame.render_widget(messages, msg_area);
    }
}
