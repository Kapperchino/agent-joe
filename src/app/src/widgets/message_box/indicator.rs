use common_models::tui_models::State;
use ratatui::prelude::{Color, Line, Modifier, Style};
use throbber_widgets_tui::{Throbber, ThrobberState};

const THROBBER_FRAME_TICKS: usize = 8;

#[derive(Default)]
pub(super) struct BusyIndicator {
    state: ThrobberState,
    ticks: usize,
}

impl BusyIndicator {
    pub(super) fn reserved_lines(&self, actor_state: &State) -> usize {
        usize::from(Self::label(actor_state).is_some())
    }

    pub(super) fn advance(&mut self, actor_state: &State) {
        if !Self::should_tick(actor_state) {
            self.reset();
            return;
        }

        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks % THROBBER_FRAME_TICKS == 0 {
            self.state.calc_next();
        }
    }

    pub(super) fn reset(&mut self) {
        self.state = ThrobberState::default();
        self.ticks = 0;
    }

    pub(super) fn render_line(&self, actor_state: &State) -> Option<Line<'static>> {
        Self::label(actor_state).map(|label| Self::throbber(label).to_line(&self.state))
    }

    fn label(actor_state: &State) -> Option<&'static str> {
        match actor_state {
            State::ThinkingStart => Some("thinking"),
            State::ToolStart => Some("working"),
            _ => None,
        }
    }

    fn should_tick(actor_state: &State) -> bool {
        matches!(
            actor_state,
            State::ThinkingStart | State::ToolStart | State::ThinkingStop | State::ToolStop
        )
    }

    fn throbber(label: &str) -> Throbber<'static> {
        Throbber::default()
            .label(label.to_string())
            .style(Style::default().fg(Color::Yellow))
            .throbber_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
    }
}
