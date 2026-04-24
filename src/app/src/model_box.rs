use commands::command::Command;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, Paragraph, Widget};
use std::str::FromStr;
use strum::EnumMessage;
use textwrap::core::display_width;

pub struct ModelBox {
    pub models: Vec<String>,
    pub efforts: Vec<String>
}

impl Widget for ModelBox {
    fn render(self, area: Rect, buf: &mut Buffer)
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
        lines.extend(self.get_lines());
        let paragraph = Paragraph::new(lines).block(Block::bordered().title("Command"));
        paragraph.render(area, buf);
    }
}

impl ModelBox {
    fn get_lines(&'_ self) -> Vec<Line<'_>> {
        let max_command_width = self
            .models
            .iter()
            .map(|command| display_width(command.as_str()))
            .max()
            .unwrap_or(0);

        self.models
            .iter()
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
