use super::*;

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
fn full_welcome_renders_ferris_in_orange_with_unclipped_hints() {
    for area in [Rect::new(0, 0, 48, 13), Rect::new(3, 2, 100, 24)] {
        let buffer = render(area);
        let rows = rows(&buffer);
        let first = rows
            .iter()
            .position(|row| row.trim() == branding::FERRIS[0].trim())
            .unwrap();
        for (offset, mascot_row) in branding::FERRIS.iter().enumerate() {
            let row = &rows[first + offset];
            assert_eq!(row.trim(), mascot_row.trim());
            let y = area.y + u16::try_from(first + offset).unwrap();
            for x in area.x..area.right() {
                let cell = &buffer[(x, y)];
                if cell.symbol() != " " {
                    assert_eq!(cell.fg, theme::ACCENT);
                    assert_eq!(cell.bg, theme::BACKGROUND);
                }
            }
        }
        assert!(rows.iter().any(|row| row.trim() == branding::TAGLINE));
        assert!(rows.iter().any(|row| row.trim() == branding::CAPTION));
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
        assert!(!rows.iter().any(|row| row.contains("_~^~^~_")), "{area:?}");
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
