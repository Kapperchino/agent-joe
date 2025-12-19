#![warn(clippy::pedantic)]

use std::time::Duration;

use color_eyre::Result;
use crossterm::event::EventStream;
use ratatui::layout::Position;
use ratatui::widgets::{List, ListItem, ListState};
use ratatui::{
    crossterm::event::{Event, KeyCode}, layout::{Alignment, Constraint, Layout},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState}
    ,
    DefaultTerminal,
    Frame,
};
use tokio_stream::StreamExt;

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
}
#[derive(Default)]
enum InputMode {
    #[default]
    Normal,
    Editing,
}

impl App {
    pub fn new() -> Self {
        Self {
            vertical_scroll_state: Default::default(),
            horizontal_scroll_state: Default::default(),
            vertical_scroll: 0,
            horizontal_scroll: 0,
            list_state: Default::default(),
            character_index: 0,
            input: "".to_string(),
            messages: vec![],
            input_mode: Default::default(),
            do_quit: false,
            auto_scroll: true,
            msg_area_height: 0,
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
            self.messages.push(self.input.clone());
            self.input.clear();
            self.reset_cursor();
            if self.auto_scroll {
                self.scroll_to_bottom();
            }
        }
    }

    pub(crate) async fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let mut events = EventStream::new();

        let period = Duration::from_secs_f32(1.0 / 120.0);
        let mut interval = tokio::time::interval(period);

        while !self.do_quit {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(|frame| self.draw(frame))?; },
                Some(Ok(event)) = events.next() => self.handle_term_event(&event),
            }
        }
        Ok(())
    }

    fn handle_term_event(&mut self, event: &Event) -> () {
        if let Event::Key(key) = event {
            match self.input_mode {
                InputMode::Normal => match key.code {
                    KeyCode::Char('e') => {
                        self.input_mode = InputMode::Editing;
                    }
                    KeyCode::Char('q') => self.do_quit = true,
                    KeyCode::Char('j') | KeyCode::Down => {
                        let max = self.max_scroll();
                        self.vertical_scroll = self.vertical_scroll.saturating_add(1).min(max);
                        self.vertical_scroll_state =
                            self.vertical_scroll_state.position(self.vertical_scroll);
                        *self.list_state.offset_mut() = self.vertical_scroll;
                        self.auto_scroll = false;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.vertical_scroll = self.vertical_scroll.saturating_sub(1);
                        self.vertical_scroll_state =
                            self.vertical_scroll_state.position(self.vertical_scroll);
                        *self.list_state.offset_mut() = self.vertical_scroll;
                        self.auto_scroll = false;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        self.horizontal_scroll = self.horizontal_scroll.saturating_sub(1);
                        self.horizontal_scroll_state = self
                            .horizontal_scroll_state
                            .position(self.horizontal_scroll);
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        self.horizontal_scroll = self.horizontal_scroll.saturating_add(1);
                        self.horizontal_scroll_state = self
                            .horizontal_scroll_state
                            .position(self.horizontal_scroll);
                    }
                    KeyCode::Char('G') => {
                        self.auto_scroll = true;
                        self.scroll_to_bottom();
                    }
                    _ => {}
                },
                InputMode::Editing => match key.code {
                    KeyCode::Enter => self.submit_message(),
                    KeyCode::Char(to_insert) => self.enter_char(to_insert),
                    KeyCode::Backspace => self.delete_char(),
                    KeyCode::Left => self.move_cursor_left(),
                    KeyCode::Right => self.move_cursor_right(),
                    KeyCode::Esc => self.input_mode = InputMode::Normal,
                    _ => {}
                },
            }
        }
    }
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();

        // Words made "loooong" to demonstrate line breaking.
        let s =
            "Veeeeeeeeeeeeeeeery    loooooooooooooooooong   striiiiiiiiiiiiiiiiiiiiiiiiiing.   ";
        let mut long_line = s.repeat(usize::from(area.width) / s.len() + 4);
        long_line.push('\n');

        let chunks = Layout::vertical([
            Constraint::Min(1),
            Constraint::Percentage(100),
            Constraint::Min(3),
        ]);

        let [top_bar_area, msg_area, input_area] = chunks.areas(frame.area());

        self.msg_area_height = msg_area.height as usize;

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
            })
            .block(Block::bordered().title("Input"));
        frame.render_widget(input, input_area);
        match self.input_mode {
            // Hide the cursor. `Frame` does this by default, so we don't need to do anything here
            InputMode::Normal => {}

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

        let messages: Vec<ListItem> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let content = Line::from(Span::raw(format!("{i}: {m}")));
                ListItem::new(content)
            })
            .collect();

        let vert_len = messages.iter().fold(0, |acc, x| acc + x.height());

        let messages = List::new(messages).block(Block::bordered().title("Messages"));

        self.vertical_scroll_state = self.vertical_scroll_state.content_length(vert_len);
        self.horizontal_scroll_state = self.horizontal_scroll_state.content_length(messages.len());

        let create_block = |title: &'static str| Block::bordered().gray().title(title.bold());

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
