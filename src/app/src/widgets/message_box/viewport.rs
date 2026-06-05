use ratatui::prelude::Line;

const HORIZONTAL_PADDING: u16 = 2;
const MIN_WRAP_WIDTH: usize = 1;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct MessageViewport {
    width: u16,
    height: u16,
}

impl MessageViewport {
    pub(super) fn update(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    pub(super) fn wrap_width(self) -> usize {
        self.width
            .saturating_sub(HORIZONTAL_PADDING)
            .max(MIN_WRAP_WIDTH as u16) as usize
    }

    pub(super) fn live_line_capacity(self, reserved_lines: usize) -> usize {
        usize::from(self.height).saturating_sub(reserved_lines)
    }

    pub(super) fn visible_lines(self, mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
        let capacity = usize::from(self.height);
        if lines.len() > capacity {
            lines.split_off(lines.len().saturating_sub(capacity))
        } else {
            lines
        }
    }
}
