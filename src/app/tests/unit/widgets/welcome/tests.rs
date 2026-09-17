use super::*;

const FERRIS_ROWS: [&str; 4] = [
    " ▄ ▄   ▄█████████▄   ▄ ▄ ",
    " █▄█ ▄██ ●     ● ██▄ █▄█ ",
    "  ▀█████  • ω •  █████▀  ",
    "     ▀█▀▀███████▀▀█▀     ",
];

fn render(area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    Welcome.render(area, &mut buffer);
    buffer
}

fn rows(buffer: &Buffer) -> Vec<String> {
    (buffer.area.y..buffer.area.bottom())
        .map(|y| {
            (buffer.area.x..buffer.area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn full_welcome_renders_filled_ferris_with_unclipped_hints() {
    assert_eq!(branding::ferris().map(|row| row.width()), [25; 4]);
    for area in [
        Rect::new(0, 0, 48, 13),
        Rect::new(0, 0, 49, 14),
        Rect::new(3, 2, 100, 24),
        Rect::new(3, 2, 101, 25),
    ] {
        let buffer = render(area);
        let rows = rows(&buffer);
        let first = rows
            .iter()
            .position(|row| row.trim() == FERRIS_ROWS[0].trim())
            .unwrap();
        let left = area.x + area.width / 2 - 25 / 2;
        for (offset, mascot_row) in FERRIS_ROWS.iter().enumerate() {
            let row = &rows[first + offset];
            assert_eq!(row.trim(), mascot_row.trim());
            let y = area.y + u16::try_from(first + offset).unwrap();
            for (column, symbol) in mascot_row.chars().enumerate() {
                let x = left + u16::try_from(column).unwrap();
                let cell = &buffer[(x, y)];
                let expected = match (offset, column) {
                    (1, 9 | 15) => theme::base().fg(theme::BACKGROUND).bg(theme::TEXT),
                    (2, 10 | 14) => theme::base()
                        .fg(ratatui::style::Color::Rgb(188, 66, 87))
                        .bg(theme::ACCENT),
                    (1 | 2, 8..=16) => theme::base().fg(theme::BACKGROUND).bg(theme::ACCENT),
                    _ => theme::base().fg(theme::ACCENT),
                };
                assert_eq!(cell.symbol(), symbol.to_string(), "{area:?} ({x}, {y})");
                assert_eq!(cell.fg, expected.fg.unwrap(), "{area:?} ({x}, {y})");
                assert_eq!(cell.bg, expected.bg.unwrap(), "{area:?} ({x}, {y})");
            }
            for x in (area.x..left).chain(left + 25..area.right()) {
                assert_eq!(buffer[(x, y)].symbol(), " ");
                assert_eq!(buffer[(x, y)].bg, theme::BACKGROUND);
            }
        }
        assert!(rows.iter().any(|row| row.trim() == branding::TAGLINE));
        assert!(rows.iter().any(|row| row.trim() == branding::CAPTION));
        for hint in [
            "Understand an unfamiliar crate",
            "Turn an idea into working Rust",
            "Find the bug. Make it better.",
        ] {
            assert!(rows.iter().any(|row| row.contains(hint)), "{area:?}");
        }
        assert!(
            rows.iter()
                .any(|row| row.contains("write a prompt") && row.contains("commands"))
        );
    }
}

#[test]
fn compact_welcome_keeps_the_mascot_and_input_hint_visible() {
    for area in [
        Rect::new(0, 0, 34, 4),
        Rect::new(0, 0, 47, 13),
        Rect::new(0, 0, 48, 12),
        Rect::new(0, 0, 100, 3),
        Rect::new(0, 0, 16, 2),
        Rect::new(0, 0, 12, 5),
    ] {
        let buffer = render(area);
        let rows = rows(&buffer);
        let title = match area.width {
            0..16 => branding::MARK,
            _ => branding::TITLE,
        };
        assert!(rows.iter().any(|row| row.trim() == title), "{area:?}");
        assert!(
            rows.iter()
                .any(|row| row.contains("i: write") || row.contains("Press i")),
            "{area:?}"
        );
        assert!(!rows.iter().any(|row| row.contains('█')), "{area:?}");
    }
}

#[test]
fn welcome_handles_zero_and_tiny_areas() {
    for width in 0..=16 {
        for height in 0..=4 {
            let area = Rect::new(0, 0, width, height);
            assert_eq!(render(area).area, area);
        }
    }
}
