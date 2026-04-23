use commands::command::{Command, CommandContext};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, Span, StatefulWidget, Style};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::str::FromStr;
use strum::EnumMessage;
use textwrap::core::display_width;

pub struct CommandBox {}

pub struct CommandBoxState {
    pub input: String,
    command_context: CommandContext,
}

impl StatefulWidget for CommandBox {
    type State = CommandBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut CommandBoxState)
    where
        Self: Sized,
    {
        let mut lines = vec![
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
        ];
        lines.extend(state.get_lines());
        let paragraph = Paragraph::new(lines).block(Block::bordered().title("Command"));
        paragraph.render(area, buf);
    }
}

impl CommandBoxState {
    pub fn new() -> CommandBoxState {
        CommandBoxState {
            input: "".to_string(),
            command_context: CommandContext::new(),
        }
    }

    fn get_lines(&'_ mut self) -> Vec<Line<'_>> {
        let commands = self.command_context.search(&self.input);
        let max_command_width = commands
            .iter()
            .map(|command| display_width(command.as_str()))
            .max()
            .unwrap_or(0);

        commands
            .into_iter()
            .map(|command_name| {
                let command = Command::from_str(&command_name).unwrap_or(Command::Clear);
                let padding = " ".repeat(
                    max_command_width.saturating_sub(display_width(command_name.as_str())) + 2,
                );

                Line::from(vec![
                    Span::styled(
                        command_name,
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(padding),
                    Span::styled(
                        command.get_message().unwrap(),
                        Style::default().fg(Color::Gray),
                    ),
                ])
            })
            .collect()
    }
}
