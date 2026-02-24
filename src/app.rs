#![warn(clippy::pedantic)]

use std::time::Duration;

use color_eyre::Result;
use crossterm::event::{EventStream, KeyEvent};
use futures::StreamExt;
use ractor::ActorRef;
use ratatui::layout::Position;
use ratatui::text::Span;
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::{
    crossterm::event::{Event, KeyCode}, layout::{Alignment, Constraint, Layout},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    DefaultTerminal,
    Frame,
};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::actor::{ActorToTui, Message, State};

pub(crate) struct App {
    pub vertical_scroll_state: ScrollbarState,
    pub horizontal_scroll_state: ScrollbarState,
    pub vertical_scroll: usize,
    pub horizontal_scroll: usize,
    pub list_state: ListState,
    character_index: usize,
    input: String,
    messages: Vec<String>,
    input_mode: InputMode,
    do_quit: bool,
    auto_scroll: bool,
    msg_area_height: usize,
    msg_area_width: usize,
    actor_ref: ActorRef<Message>,
    actor_state: State,
}
#[derive(Default)]
enum InputMode {
    #[default]
    Normal,
    InputCommand,
    Editing,
}

#[derive(Debug, Clone)]
pub enum Command {
    PrintContext,
}

impl Command {
    pub fn parse(string: &str) -> anyhow::Result<Command> {
        match string {
            "context" => Ok(Command::PrintContext),
            _ => Err(anyhow::anyhow!("Invalid command")),
        }
    }
}

impl App {
    pub fn new(actor_ref: ActorRef<Message>) -> Self {
        Self {
            vertical_scroll_state: Default::default(),
            horizontal_scroll_state: Default::default(),
            vertical_scroll: 0,
            horizontal_scroll: 0,
            list_state: Default::default(),
            character_index: 0,
            input: String::new(),
            messages: vec![],
            input_mode: Default::default(),
            do_quit: false,
            auto_scroll: true,
            msg_area_height: 0,
            msg_area_width: 0,
            actor_ref,
            actor_state: State::Ready,
        }
    }

    fn max_scroll(&self) -> usize {
        let visible_lines = self.msg_area_height.saturating_sub(2);
        self.messages.len().saturating_sub(visible_lines)
    }

    fn scroll_to_bottom(&mut self) {
        self.vertical_scroll = self.max_scroll();
        self.vertical_scroll_state = self.vertical_scroll_state.position(self.vertical_scroll);
        *self.list_state.offset_mut() = self.vertical_scroll;
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
        let cursor_moved_right = self.character_index.saturating_add(string.len());
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
            if self.auto_scroll {
                self.scroll_to_bottom();
            }
        }
    }

    fn submit_command(&mut self) {
        if !self.input.is_empty() {
            let command = Command::parse(self.input.as_str());
            match command {
                Ok(command) => {
                    match self.actor_ref.send_message(Message::Command(command)) {
                        Ok(_) => {}
                        Err(_) => {
                            eprintln!("it's joever")
                        }
                    };

                    self.messages.append(&mut self.wrap_str(&self.input));
                    self.input.clear();
                    self.reset_cursor();
                    if self.auto_scroll {
                        self.scroll_to_bottom();
                    }
                }
                Err(err) => {
                    self.messages.append(&mut self.wrap_str(&err.to_string()));
                }
            }
        }
    }

    fn wrap_str(&self, string: &String) -> Vec<String> {
        textwrap::wrap(
            string.as_str(),
            textwrap::Options::new(self.msg_area_width - 5),
        )
        .into_iter()
        .map(|x| x.to_string())
        .collect()
    }

    fn scroll_up(&mut self) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(1);
        self.vertical_scroll_state = self.vertical_scroll_state.position(self.vertical_scroll);
        *self.list_state.offset_mut() = self.vertical_scroll;
        self.auto_scroll = false;
    }

    fn scroll_down(&mut self) {
        let max = self.max_scroll();
        self.vertical_scroll = self.vertical_scroll.saturating_add(1).min(max);
        self.vertical_scroll_state = self.vertical_scroll_state.position(self.vertical_scroll);
        *self.list_state.offset_mut() = self.vertical_scroll;
        self.auto_scroll = false;
    }

    fn scroll_left(&mut self) {
        self.horizontal_scroll = self.horizontal_scroll.saturating_sub(1);
        self.horizontal_scroll_state = self
            .horizontal_scroll_state
            .position(self.horizontal_scroll);
    }

    fn scroll_right(&mut self) {
        self.horizontal_scroll = self.horizontal_scroll.saturating_add(1);
        self.horizontal_scroll_state = self
            .horizontal_scroll_state
            .position(self.horizontal_scroll);
    }

    pub(crate) async fn run(
        mut self,
        mut terminal: DefaultTerminal,
        mut actor_rx: UnboundedReceiver<ActorToTui>,
    ) -> Result<()> {
        let mut events = EventStream::new();

        let period = Duration::from_secs_f32(1.0 / 120.0);
        let mut interval = tokio::time::interval(period);

        while !self.do_quit {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(|frame| self.draw(frame))?; },
                Some(Ok(event)) = events.next() => self.handle_term_event(&event),
                Some(actor_msg) = actor_rx.recv() => self.handle_actor_msg(actor_msg),
            }
        }
        Ok(())
    }

    fn handle_actor_msg(&mut self, msg: ActorToTui) {
        match msg {
            ActorToTui::StateChanged(state) => {
                self.actor_state = state;
                match self.actor_state {
                    State::ThinkingStart => self.messages.push("\n".to_string()),
                    State::MessageStart => self.messages.push("\n".to_string()),
                    State::MessageStop => self.messages.push("\n".to_string()),
                    State::ThinkingStop => self.messages.push("\n".to_string()),
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
            ActorToTui::ToolUse(names) => {
                names.into_iter().for_each(|name| {
                    self.messages.push(format!("--- [tool: {}] ---", name));
                });
                if self.auto_scroll {
                    self.scroll_to_bottom();
                }
            }
            ActorToTui::CommandResult(_, command_res) => {
                let mut wrapped = self.wrap_str(&command_res);
                self.messages.append(&mut wrapped);
            }
        }
    }

    fn handle_term_event(&mut self, event: &Event) {
        match event {
            Event::FocusGained => {}
            Event::FocusLost => {}
            Event::Key(key) => self.handle_key_event(key),
            Event::Mouse(_) => {}
            Event::Paste(text) => {
                if let InputMode::Editing = self.input_mode {
                    self.paste(text)
                }
            }
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
                    match self.actor_ref.send_message(Message::KYS) {
                        Ok(_) => {}
                        Err(_) => {}
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => self.scroll_down(),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_up(),
                KeyCode::Char('h') | KeyCode::Left => self.scroll_left(),
                KeyCode::Char('l') | KeyCode::Right => self.scroll_right(),
                KeyCode::Char('G') => {
                    self.auto_scroll = true;
                    self.scroll_to_bottom();
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
        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Percentage(100),
            Constraint::Min(3),
        ]);

        let [top_bar_area, msg_area, input_area] = chunks.areas(frame.area());

        self.msg_area_height = msg_area.height as usize;
        self.msg_area_width = msg_area.width as usize;

        if self.vertical_scroll >= self.max_scroll() {
            self.auto_scroll = true;
        }

        if self.auto_scroll {
            self.scroll_to_bottom();
        }

        let input = Paragraph::new(self.input.as_str())
            .style(match self.input_mode {
                InputMode::Normal => Style::default(),
                InputMode::Editing => Style::default().fg(Color::Yellow),
                InputMode::InputCommand => Style::default().fg(Color::Green),
            })
            .block(Block::bordered().title("Input"));
        frame.render_widget(input, input_area);
        match self.input_mode {
            // Hide the cursor. `Frame` does this by default, so we don't need to do anything here
            InputMode::Normal => {}

            InputMode::InputCommand => {
                frame.set_cursor_position(Position::new(
                    // Draw the cursor at the current position in the input field.
                    // This position is can be controlled via the left and right arrow key
                    input_area.x + self.character_index as u16 + 1,
                    // Move one line down, from the border to the input line
                    input_area.y + 1,
                ))
            }

            // Make the cursor visible and ask ratatui to put it at the specified coordinates after
            // rendering
            #[allow(clippy::cast_possible_truncation)]
            InputMode::Editing => frame.set_cursor_position(Position::new(
                // Draw the cursor at the current position in the input field.
                // This position is can be controlled via the left and right arrow key
                input_area.x + self.character_index as u16 + 1,
                // Move one line down, from the border to the input line
                input_area.y + 1,
            )),
        }

        // Account for borders and ensure minimum width of 1 to avoid panic
        let messages: Vec<ListItem> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if m.starts_with("--- [tool:") && m.ends_with("] ---") {
                    let content =
                        Line::from(Span::styled(m.as_str(), Style::default().fg(Color::Cyan)));
                    ListItem::new(content)
                } else {
                    let content = Line::from(Span::raw(m));
                    ListItem::new(content)
                }
            })
            .collect();

        let messages = List::new(messages).block(Block::bordered().title("Messages"));

        self.vertical_scroll_state = self
            .vertical_scroll_state
            .content_length(self.max_scroll().into());
        self.horizontal_scroll_state = self.horizontal_scroll_state.content_length(messages.len());

        let title = Block::new()
            .title_alignment(Alignment::Center)
            .title("Use h j k l or ◄ ▲ ▼ ► to scroll ".bold());
        frame.render_widget(title, top_bar_area);

        frame.render_stateful_widget(messages, msg_area, &mut self.list_state);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            msg_area,
            &mut self.vertical_scroll_state,
        );
    }
}
