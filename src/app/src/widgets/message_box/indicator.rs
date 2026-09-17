use crate::{branding, theme};
use common_models::tui_models::State;
use ratatui::prelude::{Line, Modifier, Span, Style};

const FERRIS_FRAME_TICKS: usize = 24;

#[derive(Clone, Copy, Default)]
enum FerrisFrame {
    #[default]
    Raised,
    WaveLeft,
    Blink,
    WaveRight,
}

impl FerrisFrame {
    fn next(self) -> Self {
        match self {
            Self::Raised => Self::WaveLeft,
            Self::WaveLeft => Self::Blink,
            Self::Blink => Self::WaveRight,
            Self::WaveRight => Self::Raised,
        }
    }

    fn glyphs(self) -> &'static str {
        match self {
            Self::Raised => branding::MARK,
            Self::WaveLeft => "v(^_^)V",
            Self::Blink => "V(-_-)V",
            Self::WaveRight => "V(^_^)v",
        }
    }
}

#[derive(Default)]
pub(super) struct BusyIndicator {
    frame: FerrisFrame,
    ticks: usize,
}

impl BusyIndicator {
    pub(super) fn reserved_lines(&self, actor_state: &State) -> usize {
        usize::from(Self::label(actor_state).is_some())
    }

    pub(super) fn advance(&mut self, actor_state: &State) {
        match actor_state {
            State::StreamStart
            | State::ThinkingStart
            | State::ToolStart
            | State::ThinkingStop
            | State::ToolStop => {
                self.ticks = (self.ticks + 1) % FERRIS_FRAME_TICKS;
                if self.ticks == 0 {
                    self.frame = self.frame.next();
                }
            }
            _ => self.reset(),
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn render_line(&self, actor_state: &State) -> Option<Line<'static>> {
        Self::label(actor_state).map(|label| {
            Line::from(vec![
                Span::styled(
                    self.frame.glyphs(),
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(label, Style::default().fg(theme::AMBER)),
            ])
        })
    }

    fn label(actor_state: &State) -> Option<&'static str> {
        match actor_state {
            State::StreamStart => Some("Connecting…"),
            State::ThinkingStart => Some("Thinking it through…"),
            State::ToolStart => Some("Working on it…"),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/widgets/message_box/indicator/tests.rs"]
mod tests;
