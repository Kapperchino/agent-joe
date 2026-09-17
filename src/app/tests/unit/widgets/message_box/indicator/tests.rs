use super::*;
use crate::widgets::message_box::message_box::{MessageBox, MessageBoxState, Msg};
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};

struct BusyCase {
    state: State,
    label: &'static str,
}

#[test]
fn ferris_preserves_busy_labels_and_uses_the_brand_color() {
    let indicator = BusyIndicator::default();
    for case in [
        BusyCase {
            state: State::StreamStart,
            label: "Connecting…",
        },
        BusyCase {
            state: State::ThinkingStart,
            label: "Thinking it through…",
        },
        BusyCase {
            state: State::ToolStart,
            label: "Working on it…",
        },
    ] {
        let line = indicator.render_line(&case.state).unwrap();
        assert_eq!(
            line.to_string(),
            format!("{} {}", branding::MARK, case.label)
        );
        assert_eq!(indicator.reserved_lines(&case.state), 1);
        assert_eq!(line.spans[0].style.fg, Some(theme::ACCENT));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[2].style.fg, Some(theme::AMBER));
    }
}

#[test]
fn ferris_cycles_at_a_fixed_cadence_without_shifting_the_label() {
    let mut indicator = BusyIndicator::default();
    for _ in 0..3 {
        for glyphs in ["V(^_^)V", "v(^_^)V", "V(-_-)V", "V(^_^)v"] {
            for _ in 0..FERRIS_FRAME_TICKS {
                let line = indicator.render_line(&State::ThinkingStart).unwrap();
                assert_eq!(line.to_string(), format!("{glyphs} Thinking it through…"));
                assert_eq!(line.spans[0].width(), 7);
                assert_eq!(line.width(), 28);
                assert_eq!(indicator.render_line(&State::ThinkingStart), Some(line));
                indicator.advance(&State::ThinkingStart);
            }
        }
    }
    assert_eq!(indicator.frame.glyphs(), branding::MARK);
}

#[test]
fn inactive_states_hide_ferris_and_reset_the_animation() {
    for state in [
        State::Ready,
        State::StreamStop,
        State::MessageStart,
        State::MessageStop,
        State::Stopped,
    ] {
        let mut indicator = BusyIndicator::default();
        for _ in 0..FERRIS_FRAME_TICKS + 3 {
            indicator.advance(&State::ToolStart);
        }
        assert_ne!(indicator.frame.glyphs(), branding::MARK);
        assert_eq!(indicator.render_line(&state), None);
        assert_eq!(indicator.reserved_lines(&state), 0);
        indicator.advance(&state);
        assert_eq!(indicator.frame.glyphs(), branding::MARK);
        assert_eq!(indicator.ticks, 0);
    }
}

#[test]
fn hidden_busy_transitions_keep_the_animation_moving() {
    let mut indicator = BusyIndicator::default();
    for state in [State::ThinkingStop, State::ToolStop] {
        let previous = indicator.frame.glyphs();
        assert_eq!(indicator.render_line(&state), None);
        assert_eq!(indicator.reserved_lines(&state), 0);
        for _ in 0..FERRIS_FRAME_TICKS {
            indicator.advance(&state);
        }
        assert_ne!(indicator.frame.glyphs(), previous);
    }
    let frame = indicator.frame.glyphs();
    indicator.advance(&State::ToolStart);
    assert_eq!(indicator.frame.glyphs(), frame);
    assert!(indicator.render_line(&State::ToolStart).is_some());
}

fn render(area: Rect, state: &mut MessageBoxState) -> Buffer {
    state.update_width_height(area.width, area.height);
    let mut buffer = Buffer::empty(area);
    MessageBox {}.render(area, &mut buffer, state);
    buffer
}

fn row(buffer: &Buffer, y: u16) -> String {
    (buffer.area.x..buffer.area.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn message_box_animates_ferris_beside_the_label_and_clears_it_when_idle() {
    let mut state = MessageBoxState::new();
    state.update_width_height(40, 2);
    state.append(Msg::Message("Hello".into()));
    state.actor_state = State::ToolStart;
    let area = Rect::new(3, 2, 40, 2);
    let first = render(area, &mut state);
    assert_eq!(row(&first, 2).trim(), "Hello");
    assert_eq!(row(&first, 3).trim(), "V(^_^)V Working on it…");
    assert_eq!(first[(3, 3)].fg, theme::ACCENT);
    assert_eq!(first[(11, 3)].fg, theme::AMBER);
    for _ in 0..FERRIS_FRAME_TICKS {
        state.advance_busy_indicator();
    }
    let next = render(area, &mut state);
    assert_eq!(row(&next, 2), row(&first, 2));
    assert_eq!(row(&next, 3).trim(), "v(^_^)V Working on it…");
    state.actor_state = State::Ready;
    state.advance_busy_indicator();
    let idle = render(area, &mut state);
    assert_eq!(row(&idle, 2).trim(), "Hello");
    assert!(row(&idle, 3).trim().is_empty());
    state.actor_state = State::ToolStart;
    let restarted = render(area, &mut state);
    assert_eq!(row(&restarted, 3), row(&first, 3));
}

#[test]
fn clearing_messages_resets_ferris_before_the_next_conversation() {
    let mut state = MessageBoxState::new();
    state.update_width_height(40, 2);
    state.append(Msg::Message("Before".into()));
    state.actor_state = State::StreamStart;
    for _ in 0..FERRIS_FRAME_TICKS + 3 {
        state.advance_busy_indicator();
    }
    state.clear();
    state.append(Msg::Message("After".into()));
    let buffer = render(Rect::new(0, 0, 40, 2), &mut state);
    assert_eq!(row(&buffer, 0).trim(), "After");
    assert_eq!(row(&buffer, 1).trim(), "V(^_^)V Connecting…");
    state.advance_busy_indicator();
    assert_eq!(render(buffer.area, &mut state), buffer);
}

#[test]
fn ferris_stays_on_one_line_in_tiny_viewports() {
    let mut state = MessageBoxState::new();
    state.append(Msg::Message("Hello".into()));
    state.actor_state = State::ThinkingStart;
    for width in 0..=30 {
        for height in 0..=2 {
            let area = Rect::new(3, 2, width, height);
            let buffer = render(area, &mut state);
            assert_eq!(buffer.area, area);
            if height > 0 {
                let expected: String = "V(^_^)V Thinking it through…"
                    .chars()
                    .take(usize::from(width))
                    .collect();
                assert_eq!(
                    row(&buffer, area.bottom() - 1).trim_end(),
                    expected.trim_end()
                );
            }
        }
    }
}
