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
        let line = indicator.render_line(&case.state, 40).unwrap();
        assert!(
            line.to_string()
                .starts_with(&format!("{}  {}", case.label, branding::MARK))
        );
        assert_eq!(line.width(), 40);
        assert_eq!(indicator.reserved_lines(&case.state), 1);
        assert_eq!(line.spans[3].style.fg, Some(branding::SHELL));
        assert!(line.spans[3].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[0].style.fg, Some(theme::AMBER));
    }
}

#[test]
fn mini_ferris_uses_brand_orange_on_the_normal_background_in_every_frame() {
    for frame in [
        FerrisFrame::Rest,
        FerrisFrame::StepLeft,
        FerrisFrame::Land,
        FerrisFrame::StepRight,
    ] {
        let line = frame.render();
        assert_eq!(line.to_string(), frame.glyphs());
        assert_eq!(line.width(), 7);
        for span in line.spans {
            assert_eq!(
                span.style,
                theme::base()
                    .fg(branding::SHELL)
                    .add_modifier(Modifier::BOLD)
            );
        }
    }
}

#[test]
fn mini_ferris_holds_each_frame_for_thirty_six_ticks() {
    for state in [State::StreamStart, State::ThinkingStart, State::ToolStart] {
        let mut indicator = BusyIndicator::default();
        for step in 0..4 {
            let initial = indicator.render_line(&state, 40).unwrap();
            for _ in 0..35 {
                indicator.advance(&state);
                assert_eq!(indicator.render_line(&state, 40), Some(initial.clone()));
                assert_eq!(indicator.steps, step);
            }
            indicator.advance(&state);
            assert_ne!(indicator.render_line(&state, 40), Some(initial));
            assert_eq!(indicator.steps, step + 1);
        }
    }
}

#[test]
fn ferris_runs_to_both_ends_at_a_fixed_cadence_without_shifting_the_label() {
    let mut indicator = BusyIndicator::default();
    for (step, offset) in [0, 1, 2, 3, 4, 5, 4, 3, 2, 1, 0].into_iter().enumerate() {
        let glyphs = ["⋎(◕ᴗ◕)⋎", "⋏(◕ᴗ◕)⋎", "⋎(◡ᴗ◡)⋎", "⋎(◕ᴗ◕)⋏"][step % 4];
        for _ in 0..FERRIS_FRAME_TICKS {
            let line = indicator.render_line(&State::ThinkingStart, 34).unwrap();
            assert_eq!(
                line.to_string(),
                format!(
                    "Thinking it through…  {}{glyphs}{}",
                    " ".repeat(offset),
                    " ".repeat(5 - offset)
                )
            );
            assert_eq!(line.spans[3].width(), 7);
            assert_eq!(line.width(), 34);
            assert_eq!(indicator.render_line(&State::ThinkingStart, 34), Some(line));
            indicator.advance(&State::ThinkingStart);
        }
    }
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
        assert_eq!(indicator.render_line(&state, 40), None);
        assert_eq!(indicator.reserved_lines(&state), 0);
        indicator.advance(&state);
        assert_eq!(indicator.frame.glyphs(), branding::MARK);
        assert_eq!(indicator.ticks, 0);
        assert_eq!(indicator.steps, 0);
    }
}

#[test]
fn hidden_busy_transitions_keep_the_animation_moving() {
    let mut indicator = BusyIndicator::default();
    for state in [State::ThinkingStop, State::ToolStop] {
        let previous = indicator.steps;
        assert_eq!(indicator.render_line(&state, 40), None);
        assert_eq!(indicator.reserved_lines(&state), 0);
        for _ in 0..FERRIS_FRAME_TICKS {
            indicator.advance(&state);
        }
        assert_eq!(indicator.steps, previous + 1);
    }
    let frame = indicator.frame.glyphs();
    indicator.advance(&State::ToolStart);
    assert_eq!(indicator.frame.glyphs(), frame);
    assert!(indicator.render_line(&State::ToolStart, 40).is_some());
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
fn message_box_moves_ferris_across_the_line_and_clears_it_when_idle() {
    let mut state = MessageBoxState::new();
    state.update_width_height(40, 2);
    state.append(Msg::Message("Hello".into()));
    state.actor_state = State::ToolStart;
    let area = Rect::new(3, 2, 40, 2);
    let first = render(area, &mut state);
    assert_eq!(row(&first, 2).trim(), "Hello");
    assert_eq!(row(&first, 3), "Working on it…  ⋎(◕ᴗ◕)⋎                 ");
    assert_eq!(first[(19, 3)].fg, branding::SHELL);
    assert_eq!(first[(3, 3)].fg, theme::AMBER);
    for x in 19..=25 {
        assert_eq!(first[(x, 3)].fg, branding::SHELL);
        assert_eq!(first[(x, 3)].bg, theme::BACKGROUND);
    }
    for _ in 0..FERRIS_FRAME_TICKS {
        state.advance_busy_indicator();
    }
    let next = render(area, &mut state);
    assert_eq!(row(&next, 2), row(&first, 2));
    assert_eq!(row(&next, 3), "Working on it…   ⋏(◕ᴗ◕)⋎                ");
    assert_eq!(next[(19, 3)].symbol(), " ");
    assert_eq!(next[(19, 3)].bg, theme::BACKGROUND);
    for x in 20..=26 {
        assert_eq!(next[(x, 3)].fg, branding::SHELL);
        assert_eq!(next[(x, 3)].bg, theme::BACKGROUND);
    }
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
    assert_eq!(row(&buffer, 1), "Connecting…  ⋎(◕ᴗ◕)⋎                    ");
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
                let expected = match width {
                    0..29 => "⋎(◕ᴗ◕)⋎ Thinking it through…"
                        .chars()
                        .take(usize::from(width))
                        .collect(),
                    _ => format!(
                        "Thinking it through…  ⋎(◕ᴗ◕)⋎{}",
                        " ".repeat(usize::from(width) - 29)
                    ),
                };
                assert_eq!(
                    row(&buffer, area.bottom() - 1).trim_end(),
                    expected.trim_end()
                );
            }
        }
    }
}

#[test]
fn running_ferris_fits_after_resizing_and_changing_busy_states() {
    let mut indicator = BusyIndicator::default();
    for _ in 0..FERRIS_FRAME_TICKS * 83 {
        indicator.advance(&State::ThinkingStart);
    }
    for state in [State::StreamStart, State::ThinkingStart, State::ToolStart] {
        for width in [80, 29, 40, 120, 30] {
            let line = indicator.render_line(&state, width).unwrap();
            assert_eq!(line.width(), usize::from(width));
            assert!(
                line.to_string()
                    .starts_with(BusyIndicator::label(&state).unwrap())
            );
            assert_eq!(line.spans[3].content.as_ref(), indicator.frame.glyphs());
        }
    }
}
