use crate::{branding, theme};
use common_models::tui_models::State;
use ratatui::prelude::{Line, Modifier, Span, Style};

const FERRIS_FRAME_TICKS: usize = 36;

#[derive(Clone, Copy, Default)]
enum FerrisFrame {
    #[default]
    Rest,
    StepLeft,
    Land,
    StepRight,
}

impl FerrisFrame {
    fn next(self) -> Self {
        match self {
            Self::Rest => Self::StepLeft,
            Self::StepLeft => Self::Land,
            Self::Land => Self::StepRight,
            Self::StepRight => Self::Rest,
        }
    }

    fn glyphs(self) -> &'static str {
        match self {
            Self::Rest => branding::MARK,
            Self::StepLeft => "⋏(◕ᴗ◕)⋎",
            Self::Land => "⋎(◡ᴗ◡)⋎",
            Self::StepRight => "⋎(◕ᴗ◕)⋏",
        }
    }

    fn render(self) -> Line<'static> {
        Line::from(Span::styled(
            self.glyphs(),
            theme::base()
                .fg(branding::SHELL)
                .add_modifier(Modifier::BOLD),
        ))
    }
}

#[derive(Default)]
pub(super) struct BusyIndicator {
    frame: FerrisFrame,
    ticks: usize,
    steps: usize,
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
                    self.steps = self.steps.wrapping_add(1);
                }
            }
            _ => self.reset(),
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn render_line(&self, actor_state: &State, width: u16) -> Option<Line<'static>> {
        Self::label(actor_state).map(|label| {
            let ferris = self.frame.render();
            let label = Span::styled(label, Style::default().fg(theme::AMBER));
            match usize::from(width).checked_sub(label.width() + ferris.width() + 2) {
                Some(travel) => {
                    let phase = self.steps % (travel * 2).max(1);
                    let offset = travel - travel.abs_diff(phase);
                    Line::from(
                        [label, Span::raw("  "), Span::raw(" ".repeat(offset))]
                            .into_iter()
                            .chain(ferris.spans)
                            .chain([Span::raw(" ".repeat(travel - offset))])
                            .collect::<Vec<_>>(),
                    )
                }
                None => Line::from(
                    ferris
                        .spans
                        .into_iter()
                        .chain([Span::raw(" "), label])
                        .collect::<Vec<_>>(),
                ),
            }
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
