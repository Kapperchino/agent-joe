use crate::widgets::command_box::CommandBox;
use crate::tui::{HomeMenu, InputMode};
use commands::command::CommandContext;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget};

pub struct InputBox {}

pub struct InputBoxState {
    character_index: usize,
    input: String,
    pub input_mode: InputMode,
    command_context: CommandContext,
}

impl StatefulWidget for InputBox {
    type State = InputBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut InputBoxState)
    where
        Self: Sized,
    {
        let input_wrap_width = InputBoxState::input_wrap_width(area.width);
        let editing_display_lines =
            InputBoxState::wrap_input_text(&state.input, input_wrap_width, "", "").len();
        let command_display_lines = InputBoxState::wrap_input_text(
            &state.input,
            input_wrap_width,
            InputBoxState::COMMAND_PROMPT,
            InputBoxState::COMMAND_CONTINUATION,
        )
        .len();

        let editing_cursor = state.cursor_wrap_position(input_wrap_width, "", "");
        let command_cursor = state.cursor_wrap_position(
            input_wrap_width,
            InputBoxState::COMMAND_PROMPT,
            InputBoxState::COMMAND_CONTINUATION,
        );
        let editing_input_lines = editing_display_lines.max(usize::from(editing_cursor.1) + 1);
        let command_input_lines = command_display_lines.max(usize::from(command_cursor.1) + 1);

        match &state.input_mode {
            InputMode::HomeMenu(home) => match home {
                HomeMenu::InputCommand => {
                    let command_input_height = command_input_lines as u16 + 2;
                    let [command_input_area, command_box_area] = Layout::vertical([
                        Constraint::Length(command_input_height),
                        Constraint::Min(3),
                    ])
                    .areas(area);

                    let command_section =
                        Paragraph::new(state.command_lines(input_wrap_width, command_input_lines))
                            .block(Block::bordered().title("Command"));
                    command_section.render(command_input_area, buf);
                    CommandBox {
                        commands: state.command_context.search(&state.input),
                    }
                    .render(command_box_area, buf);
                }
                _ => {
                    let input =
                        Paragraph::new(state.input_lines(input_wrap_width, editing_input_lines))
                            .style(match &state.input_mode {
                                InputMode::HomeMenu(home) => match home {
                                    HomeMenu::Normal => Style::default(),
                                    HomeMenu::Editing => Style::default().fg(Color::Yellow),
                                    HomeMenu::InputCommand => Style::default().fg(Color::Green),
                                },
                                InputMode::CommandMenu(_) | InputMode::None => Style::default(),
                            })
                            .block(Block::bordered().title("Input"));
                    input.render(area, buf);
                }
            },
            InputMode::CommandMenu(_) => todo!(),
            InputMode::None => (),
        }
    }
}

impl InputBoxState {
    const COMMAND_PROMPT: &'static str = "/ ";
    const COMMAND_CONTINUATION: &'static str = "   ";

    pub fn new() -> InputBoxState {
        InputBoxState {
            character_index: 0,
            input: "".to_string(),
            input_mode: Default::default(),
            command_context: CommandContext::new(),
        }
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.reset_cursor();
    }

    pub fn get_input(&self) -> String {
        self.input.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    pub fn get_cursor_pos(&self, area: &Rect) -> Position {
        let input_wrap_width = Self::input_wrap_width(area.width);
        let (cursor_x, cursor_y) = match &self.input_mode {
            InputMode::HomeMenu(home) => match home {
                HomeMenu::InputCommand => self.cursor_wrap_position(
                    input_wrap_width,
                    Self::COMMAND_PROMPT,
                    Self::COMMAND_CONTINUATION,
                ),
                HomeMenu::Normal | HomeMenu::Editing => {
                    self.cursor_wrap_position(input_wrap_width, "", "")
                }
            },
            InputMode::CommandMenu(_) | InputMode::None => {
                self.cursor_wrap_position(input_wrap_width, "", "")
            }
        };

        Position::new(area.x + cursor_x + 1, area.y + cursor_y + 1)
    }

    pub fn get_height(&self, width: u16) -> u16 {
        let input_wrap_width = Self::input_wrap_width(width);
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
        if matches!(self.input_mode, InputMode::HomeMenu(HomeMenu::InputCommand)) {
            command_input_lines as u16 + 9
        } else {
            editing_input_lines as u16 + 2
        }
    }

    pub(crate) fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.character_index.saturating_sub(1);
        self.character_index = self.clamp_cursor(cursor_moved_left);
    }

    pub(crate) fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.character_index.saturating_add(1);
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    pub fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
    }

    pub(crate) fn paste(&mut self, string: &String) {
        self.input.push_str(string);
        let cursor_moved_right = self.character_index.saturating_add(string.chars().count());
        self.character_index = self.clamp_cursor(cursor_moved_right);
    }

    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    fn reset_cursor(&mut self) {
        self.character_index = 0;
    }

    pub fn auto_comp_command(&mut self) {
        self.command_context.search(&self.input).first().map(|cmd| {
            self.input = cmd.clone();
            self.character_index = self.clamp_cursor(cmd.len());
        });
    }

    pub(crate) fn delete_char(&mut self) {
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

    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.character_index)
            .unwrap_or(self.input.len())
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
                Style::default(),
            ),
        ])];

        lines.extend(
            command_input
                .into_iter()
                .map(|line| Line::from(Span::styled(line, Style::default()))),
        );
        lines
    }
    fn input_lines(&self, wrap_width: usize, min_lines: usize) -> Vec<Line<'static>> {
        let mut lines = Self::wrap_input_text(&self.input, wrap_width, "", "");
        lines.resize_with(min_lines.max(1), String::new);
        lines.into_iter().map(Line::from).collect()
    }
}
