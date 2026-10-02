use super::render::{TranscriptLine, TranscriptRenderState, TranscriptRenderer};
use ratatui::prelude::Line;

pub(super) struct ScrollbackRenderer {
    renderer: TranscriptRenderer,
    state: TranscriptRenderState,
}

impl ScrollbackRenderer {
    pub(super) fn new() -> Self {
        Self {
            renderer: TranscriptRenderer::new(1),
            state: TranscriptRenderState::default(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.state = TranscriptRenderState::default();
    }

    pub(super) fn update_width(&mut self, width: usize) {
        self.renderer.update_width(width);
    }

    pub(super) fn render_flushed_lines(&mut self, lines: &[TranscriptLine]) -> Vec<Line<'static>> {
        self.renderer.render_lines(lines, &mut self.state)
    }

    pub(super) fn render_history_lines(&self, lines: &[TranscriptLine]) -> Vec<Line<'static>> {
        self.renderer
            .render_lines(lines, &mut TranscriptRenderState::default())
    }

    pub(super) fn render_live_lines(
        &self,
        committed_lines: &[TranscriptLine],
        active_lines: Option<Vec<TranscriptLine>>,
        status_line: Option<Line<'static>>,
    ) -> Vec<Line<'static>> {
        let mut render_state = self.state.clone();
        let mut lines = self
            .renderer
            .render_lines(committed_lines, &mut render_state);

        if let Some(active_lines) = active_lines {
            lines.extend(self.renderer.render_lines(&active_lines, &mut render_state));
        }

        if let Some(line) = status_line {
            lines.push(line);
        }

        lines
    }
}
