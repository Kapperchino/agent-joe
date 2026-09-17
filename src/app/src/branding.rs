use crate::theme;
use ratatui::{
    style::Color,
    text::{Line, Span},
};

pub(crate) const MARK: &str = "v(•ᴗ•)v";
pub(crate) const TITLE: &str = "v(•ᴗ•)v joe code";
pub(crate) const TAGLINE: &str = "Small claws. Big ideas.";
pub(crate) const CAPTION: &str = "Ferris, your Rust companion.";
pub(crate) const BLUSH: Color = Color::Rgb(203, 76, 100);

pub(crate) fn ferris() -> [Line<'static>; 4] {
    let shell = theme::base().fg(theme::ACCENT);
    let face = theme::base().fg(theme::BACKGROUND).bg(theme::ACCENT);
    let eyes = face.fg(theme::TEXT);
    let blush = face.fg(BLUSH);

    [
        Line::from(Span::styled(" ▄ ▄    ▄███████▄    ▄ ▄ ", shell)),
        Line::from(vec![
            Span::styled(" █▄█  ▄█", shell),
            Span::styled(" ", face),
            Span::styled("◕", eyes),
            Span::styled("     ", face),
            Span::styled("◕", eyes),
            Span::styled(" ", face),
            Span::styled("█▄  █▄█ ", shell),
        ]),
        Line::from(vec![
            Span::styled("  ▀█████", shell),
            Span::styled(" ", face),
            Span::styled("˶", blush),
            Span::styled("  ᴗ  ", face),
            Span::styled("˶", blush),
            Span::styled(" ", face),
            Span::styled("█████▀  ", shell),
        ]),
        Line::from(Span::styled("     ▀█▄█▀▀▀▀▀▀▀█▄█▀     ", shell)),
    ]
}
