use commands::command::Command;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Line, Modifier, Span, Style};
use ratatui::widgets::{Paragraph, Widget};
use std::str::FromStr;
use strum::EnumMessage;
use textwrap::core::display_width;

pub struct CommandBox {
    pub commands: Vec<String>,
}

impl Widget for CommandBox {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let lines = match self.commands.is_empty() {
            true => vec![Line::from(theme::muted(
                "No matching commands. Try a different search.",
            ))],
            false => self.get_lines(),
        };
        let paragraph = Paragraph::new(lines).block(theme::panel("Commands", theme::BORDER));
        paragraph.render(area, buf);
    }
}

impl CommandBox {
    fn get_lines(&'_ self) -> Vec<Line<'_>> {
        let max_command_width = self
            .commands
            .iter()
            .map(|command| display_width(command.as_str()))
            .max()
            .unwrap_or(0);

        self.commands
            .iter()
            .map(|command_name| {
                let command = Command::from_str(&command_name).unwrap_or(Command::Clear);
                let padding = " ".repeat(
                    max_command_width.saturating_sub(display_width(command_name.as_str())) + 2,
                );

                Line::from(vec![
                    Span::styled(" /", Style::default().fg(theme::AMBER)),
                    Span::styled(
                        command_name,
                        Style::default()
                            .fg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(padding),
                    Span::styled(
                        command.get_message().unwrap(),
                        Style::default().fg(theme::MUTED),
                    ),
                ])
            })
            .collect()
    }
}
use crate::theme;
