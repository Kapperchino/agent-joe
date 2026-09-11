use crate::theme;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

pub(crate) struct Welcome;

impl Widget for Welcome {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = match (area.width, area.height) {
            (0..34, _) => vec![
                Line::from(Span::styled(
                    "Ready when you are.",
                    Style::default().fg(theme::ACCENT),
                )),
                Line::from(theme::muted("Press i to begin.")),
            ],
            (34..48, _) | (_, 0..12) => vec![
                Line::from(Span::styled(
                    "A little Joe. A lot of possibility.",
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(theme::muted("Press i to start building.")),
            ],
            _ => vec![
                Line::from(Span::styled(
                    "▄▄▄  ▄▄▄  ▄▄▄",
                    Style::default().fg(theme::ACCENT),
                )),
                Line::from(Span::styled(
                    " ▐█  █ █  █▄ ",
                    Style::default().fg(theme::ACCENT),
                )),
                Line::from(Span::styled(
                    "▀▀   ▀▀▀  ▀▄▄",
                    Style::default().fg(theme::ACCENT),
                )),
                Line::default(),
                Line::from(Span::styled(
                    "A little Joe. A lot of possibility.",
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(theme::muted("Your Rust workspace, ready for what’s next.")),
                Line::default(),
                Line::from(vec![
                    Span::styled(" 01 ", Style::default().fg(theme::AMBER)),
                    theme::muted("Explore   "),
                    Span::raw("Understand an unfamiliar crate"),
                ]),
                Line::from(vec![
                    Span::styled(" 02 ", Style::default().fg(theme::AMBER)),
                    theme::muted("Build     "),
                    Span::raw("Turn an idea into working Rust"),
                ]),
                Line::from(vec![
                    Span::styled(" 03 ", Style::default().fg(theme::AMBER)),
                    theme::muted("Refine    "),
                    Span::raw("Find the bug. Make it better."),
                ]),
                Line::default(),
                theme::hints(
                    &[
                        theme::KeyHint::new("i", "write a prompt"),
                        theme::KeyHint::new("/", "commands"),
                    ],
                    area.width,
                ),
            ],
        };
        let [content] = Layout::vertical([Constraint::Length(
            u16::try_from(lines.len()).unwrap_or(u16::MAX),
        )])
        .flex(Flex::Center)
        .areas(area);
        Paragraph::new(lines)
            .style(theme::base())
            .centered()
            .render(content, buf);
    }
}
