use super::*;

fn luminance(color: Color) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        match value {
            value if value <= 0.04045 => value / 12.92,
            value => ((value + 0.055) / 1.055).powf(2.4),
        }
    };
    match color {
        Color::Rgb(red, green, blue) => {
            0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
        }
        _ => panic!("The Ferris palette must use explicit RGB colors"),
    }
}

#[test]
fn text_colors_remain_readable_on_all_surfaces() {
    for foreground in [TEXT, MUTED, ACCENT, AMBER, RED, GREEN] {
        for background in [BACKGROUND, SURFACE, SELECTION] {
            let contrast = (luminance(foreground) + 0.05) / (luminance(background) + 0.05);
            assert!(
                contrast >= 4.5,
                "{foreground:?} on {background:?}: {contrast}"
            );
        }
    }
}

#[test]
fn ferris_orange_is_distinct_from_semantic_status_colors() {
    assert_eq!(ACCENT, Color::Rgb(255, 153, 102));
    for color in [GREEN, AMBER, RED] {
        assert_ne!(ACCENT, color);
    }
    let badge = badge("BUILD", ACCENT);
    assert_eq!(badge.style.fg, Some(BACKGROUND));
    assert_eq!(badge.style.bg, Some(ACCENT));
    assert!(badge.style.add_modifier.contains(Modifier::BOLD));
}
