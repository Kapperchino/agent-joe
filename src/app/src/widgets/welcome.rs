use crate::{branding, theme};
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
            (0..34, _) | (_, 0..4) => vec![
                Line::from(Span::styled(
                    match area.width {
                        0..16 => branding::MARK,
                        _ => branding::TITLE,
                    },
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(theme::muted("i: write")),
            ],
            (34..48, _) | (_, 4..13) => vec![
                Line::from(Span::styled(
                    branding::TITLE,
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(branding::TAGLINE),
                Line::from(theme::muted(branding::CAPTION)),
                Line::from(theme::muted("Press i to start building.")),
            ],
            _ => branding::FERRIS
                .into_iter()
                .map(|row| Line::from(Span::styled(row, Style::default().fg(theme::ACCENT))))
                .chain([
                    Line::default(),
                    Line::from(Span::styled(
                        branding::TAGLINE,
                        Style::default()
                            .fg(theme::TEXT)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(theme::muted(branding::CAPTION)),
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
                ])
                .collect(),
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

#[cfg(test)]
#[path = "../../tests/unit/widgets/welcome/tests.rs"]
mod tests;
