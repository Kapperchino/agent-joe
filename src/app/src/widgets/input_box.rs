use crate::models::{EffortsSelection, ModelSelections};
use crate::tui::{CommandMenu, HomeMenu, InputMode};
use crate::widgets::command_box::CommandBox;
use crate::widgets::model_box::{ModelBox, ModelBoxResult, ModelBoxState};
use clients::config::Config;
use commands::command::CommandContext;
use crossterm::event::KeyEvent;
use hjkl_engine::{DefaultHost, Editor, InsertDir, Options, VimMode};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Paragraph, StatefulWidget, Widget};

pub struct InputBox {
    model_box: ModelBox,
}

pub struct InputBoxState {
    editor: Editor<hjkl_buffer::Buffer, DefaultHost>,
    pub input_mode: InputMode,
    command_context: CommandContext,
    model_box_state: ModelBoxState,
}

impl StatefulWidget for InputBox {
    type State = InputBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut InputBoxState)
    where
        Self: Sized,
    {
        let input_wrap_width = InputBoxState::input_wrap_width(area.width);
        let input = state.get_input();
        let editing_display_lines =
            InputBoxState::wrap_input_text(&input, input_wrap_width, "", "").len();
        let command_display_lines = InputBoxState::wrap_input_text(
            &input,
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
                        commands: state.command_context.search(&input),
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
            InputMode::CommandMenu(menu) => match menu {
                CommandMenu::ModelSelector => {
                    self.model_box.render(area, buf, &mut state.model_box_state)
                }
            },
            InputMode::None => (),
        }
    }
}

impl InputBox {
    pub fn new() -> InputBox {
        InputBox {
            model_box: ModelBox::new(),
        }
    }
}

impl InputBoxState {
    const COMMAND_PROMPT: &'static str = "/ ";
    const COMMAND_CONTINUATION: &'static str = "   ";

    pub fn new(config: Config) -> InputBoxState {
        let model_box_state = match config {
            Config::Claude(_) => ModelBoxState::new(
                ModelSelections::Claude,
                EffortsSelection::Claude,
                config.get_model(),
                config.get_effort(),
            ),
            Config::OpenAI(_) => ModelBoxState::new(
                ModelSelections::OpenAI,
                EffortsSelection::OpenAI,
                config.get_model(),
                config.get_effort(),
            ),
        };
        InputBoxState {
            editor: Editor::new(
                hjkl_buffer::Buffer::new(),
                DefaultHost::new(),
                Options::default(),
            ),
            input_mode: Default::default(),
            command_context: CommandContext::new(),
            model_box_state,
        }
    }

    pub fn clear(&mut self) {
        self.set_input("");
        self.reset_cursor();
    }

    pub fn get_input(&self) -> String {
        self.editor.buffer().as_string()
    }

    pub fn is_empty(&self) -> bool {
        self.get_input().is_empty()
    }

    pub(crate) fn handle_hjkl_key(&mut self, key: KeyEvent) -> bool {
        hjkl_vim::handle_key(&mut self.editor, key)
    }

    pub(crate) fn force_normal_mode(&mut self) {
        self.editor.force_normal();
    }

    pub(crate) fn is_insert_mode(&self) -> bool {
        self.editor.vim_mode() == VimMode::Insert
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
        let input = self.get_input();
        let editing_display_lines = Self::wrap_input_text(&input, input_wrap_width, "", "").len();
        let command_display_lines = Self::wrap_input_text(
            &input,
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
        match self.input_mode {
            InputMode::HomeMenu(HomeMenu::InputCommand) => command_input_lines as u16 + 9,
            InputMode::CommandMenu(CommandMenu::ModelSelector) => self.model_box_state.height(),
            InputMode::HomeMenu(HomeMenu::Normal | HomeMenu::Editing) | InputMode::None => {
                editing_input_lines as u16 + 2
            }
        }
    }

    pub(crate) fn move_cursor_left(&mut self) {
        self.editor.insert_arrow(InsertDir::Left);
    }

    pub(crate) fn move_cursor_right(&mut self) {
        self.editor.insert_arrow(InsertDir::Right);
    }

    pub fn enter_char(&mut self, new_char: char) {
        if new_char == '\n' {
            self.editor.insert_newline();
        } else {
            self.editor.insert_char(new_char);
        }
    }

    pub(crate) fn paste(&mut self, string: &str) {
        self.editor.insert_str(string);
    }

    fn reset_cursor(&mut self) {
        self.editor.jump_cursor(0, 0);
    }

    pub fn auto_comp_command(&mut self) {
        if let Some(cmd) = self.command_context.search(&self.get_input()).first() {
            self.set_input(cmd);
        }
    }

    pub fn select_next_model(&mut self) {
        self.model_box_state.on_arrow_down();
    }

    pub fn select_previous_model(&mut self) {
        self.model_box_state.on_arrow_up();
    }

    pub fn confirm_select_model(&mut self) -> ModelBoxResult {
        self.model_box_state.on_enter()
    }

    pub(crate) fn delete_char(&mut self) {
        self.editor.insert_backspace();
    }

    fn set_input(&mut self, input: &str) {
        self.editor.set_content(input);
        let last_row = self.editor.buffer().row_count().saturating_sub(1);
        let last_col = self
            .editor
            .buffer()
            .line(last_row)
            .map(str::chars)
            .map(|chars| chars.count())
            .unwrap_or_default();
        self.editor.jump_cursor(last_row, last_col);
    }

    fn character_index(&self) -> usize {
        let (row, col) = self.editor.cursor();
        let previous_chars = self
            .editor
            .buffer()
            .lines()
            .iter()
            .take(row)
            .map(|line| line.chars().count() + 1)
            .sum::<usize>();
        previous_chars + col
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
        let input = self.get_input();
        let cursor_prefix: String = input.chars().take(self.character_index()).collect();
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
        let input = self.get_input();
        let mut command_input = Self::wrap_input_text(
            &input,
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
        let input = self.get_input();
        let mut lines = Self::wrap_input_text(&input, wrap_width, "", "");
        lines.resize_with(min_lines.max(1), String::new);
        lines.into_iter().map(Line::from).collect()
    }
}
