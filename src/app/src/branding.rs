use crate::theme;
use ratatui::{
    style::Color,
    text::{Line, Span},
};

pub(crate) const MARK: &str = "⋎(◕ᴗ◕)⋎";
pub(crate) const TITLE: &str = "⋎(◕ᴗ◕)⋎ joe code";
pub(crate) const TAGLINE: &str = "Small claws. Big ideas.";
pub(crate) const CAPTION: &str = "Ferris, your Rust companion.";
pub(crate) const SHELL: Color = theme::ACCENT;
pub(crate) const EYES: Color = Color::Rgb(0, 0, 0);

pub(crate) fn ferris() -> [Line<'static>; 10] {
    let shell = theme::base().fg(SHELL);
    let eyes = shell.fg(EYES).bg(SHELL);
    let glint = shell.fg(Color::Rgb(255, 255, 255)).bg(EYES);
    let claws = shell.fg(theme::BORDER).bg(SHELL);

    [
        Line::from(Span::styled("            ▄▖▗▄ ▗▖            ", shell)),
        Line::from(Span::styled("       ▄▄▟█████████▟█▖▄▖       ", shell)),
        Line::from(Span::styled("     ▄▄████████████████▙▄▖     ", shell)),
        Line::from(Span::styled("   ▗▄▟███████████████████▙▄▖   ", shell)),
        Line::from(Span::styled("  ▄▄███████████████████████▄▖  ", shell)),
        Line::from(vec![
            Span::styled("  ▐█████████", shell),
            Span::styled("▟", eyes),
            Span::styled("●", glint),
            Span::styled("▙", eyes),
            Span::styled("██", shell),
            Span::styled("▟", eyes),
            Span::styled("●", glint),
            Span::styled("▙", eyes),
            Span::styled("████████   ", shell),
        ]),
        Line::from(vec![
            Span::styled("▗▟██████████", shell),
            Span::styled("▝▀▘", eyes),
            Span::styled("██", shell),
            Span::styled("▝▀▘", eyes),
            Span::styled("█████████▙▖", shell),
        ]),
        Line::from(vec![
            Span::styled(" ▜█▖▜▞▜██", shell),
            Span::styled("╭────╮╭────╮", claws),
            Span::styled("████▘▟▘▟▛ ", shell),
        ]),
        Line::from(Span::styled("  ▝▀▄▝ ▝▀▜██▛▀     ███▀▘   ▟▘  ", shell)),
        Line::from(Span::styled("     ▘    ▝▀█▛▘ ▝▀▀▀▘     ▝    ", shell)),
    ]
}
