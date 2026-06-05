use crate::utils::draw_line::{DrawLine, RenderState};
use ratatui::prelude::Line;

pub(super) struct ScrollbackRenderer {
    draw_line: DrawLine,
    state: RenderState,
}

impl ScrollbackRenderer {
    pub(super) fn new() -> Self {
        Self {
            draw_line: DrawLine::new(),
            state: RenderState::default(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.state = RenderState::default();
    }

    pub(super) fn render_flushed_lines(&mut self, lines: &[String]) -> Vec<Line<'static>> {
        self.draw_line
            .render_lines_with_state(lines, &mut self.state)
    }

    pub(super) fn render_live_lines(
        &self,
        committed_lines: &[String],
        active_lines: Option<Vec<String>>,
        status_line: Option<Line<'static>>,
    ) -> Vec<Line<'static>> {
        let mut render_state = self.state.clone();
        let mut lines = self
            .draw_line
            .render_lines_with_state(committed_lines, &mut render_state);

        if let Some(active_lines) = active_lines {
            lines.extend(
                self.draw_line
                    .render_lines_with_state(&active_lines, &mut render_state),
            );
        }

        if let Some(line) = status_line {
            lines.push(line);
        }

        lines
    }
}
