use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType},
};

pub(crate) const BACKGROUND: Color = Color::Rgb(18, 24, 28);
pub(crate) const SURFACE: Color = Color::Rgb(25, 34, 39);
pub(crate) const SELECTION: Color = Color::Rgb(35, 57, 55);
pub(crate) const BORDER: Color = Color::Rgb(62, 82, 88);
pub(crate) const TEXT: Color = Color::Rgb(225, 231, 224);
pub(crate) const MUTED: Color = Color::Rgb(145, 164, 169);
pub(crate) const ACCENT: Color = Color::Rgb(131, 213, 184);
pub(crate) const AMBER: Color = Color::Rgb(232, 189, 120);
pub(crate) const RED: Color = Color::Rgb(239, 145, 145);

pub(crate) fn base() -> Style {
    Style::default().fg(TEXT).bg(BACKGROUND)
}

pub(crate) fn panel(title: impl Into<String>, color: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(base().bg(SURFACE))
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::default()
                .fg(match color {
                    BORDER => MUTED,
                    _ => color,
                })
                .add_modifier(Modifier::BOLD),
        ))
}

pub(crate) fn badge(label: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        format!(" {} ", label.into()),
        Style::default()
            .fg(BACKGROUND)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

pub(crate) fn muted(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(MUTED))
}

pub(crate) struct KeyHint {
    key: &'static str,
    action: &'static str,
}

impl KeyHint {
    pub(crate) fn new(key: &'static str, action: &'static str) -> Self {
        Self { key, action }
    }

    fn line(&self) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!(" {} ", self.key),
                Style::default().fg(TEXT).bg(SELECTION),
            ),
            muted(format!(" {}  ", self.action)),
        ])
    }
}

pub(crate) fn hints(items: &[KeyHint], width: u16) -> Line<'static> {
    items.iter().fold(Line::default(), |mut line, item| {
        let hint = item.line();
        if line.width() + hint.width() <= usize::from(width) {
            line.spans.extend(hint.spans);
        }
        line
    })
}

pub(crate) fn compact_number(value: u64) -> String {
    match value {
        0..1_000 => value.to_string(),
        1_000..1_000_000 => format!("{}.{}k", value / 1_000, value % 1_000 / 100),
        _ => format!("{}.{}m", value / 1_000_000, value % 1_000_000 / 100_000),
    }
}
