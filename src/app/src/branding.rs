use crate::theme;
use ratatui::{
    style::Color,
    text::{Line, Span},
};

pub(crate) const MARK: &str = "V(^_^)V";
pub(crate) const TITLE: &str = "V(^_^)V joe code";
pub(crate) const TAGLINE: &str = "Small claws. Big ideas.";
pub(crate) const CAPTION: &str = "Ferris, your Rust companion.";

pub(crate) fn ferris() -> [Line<'static>; 4] {
    let shell = theme::base().fg(theme::ACCENT);
    let face = theme::base().fg(theme::BACKGROUND).bg(theme::ACCENT);
    let eyes = face.bg(theme::TEXT);
    let blush = face.fg(Color::Rgb(188, 66, 87));

    [
        Line::from(Span::styled(" ▄ ▄   ▄█████████▄   ▄ ▄ ", shell)),
        Line::from(vec![
            Span::styled(" █▄█ ▄██", shell),
            Span::styled(" ", face),
            Span::styled("●", eyes),
            Span::styled("     ", face),
            Span::styled("●", eyes),
            Span::styled(" ", face),
            Span::styled("██▄ █▄█ ", shell),
        ]),
        Line::from(vec![
            Span::styled("  ▀█████", shell),
            Span::styled("  ", face),
            Span::styled("•", blush),
            Span::styled(" ω ", face),
            Span::styled("•", blush),
            Span::styled("  ", face),
            Span::styled("█████▀  ", shell),
        ]),
        Line::from(Span::styled("     ▀█▀▀███████▀▀█▀     ", shell)),
    ]
}
