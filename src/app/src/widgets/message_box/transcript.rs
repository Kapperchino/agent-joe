use super::format::MessageFormatter;
use super::table_flow;
use crate::widgets::message_box::message_box::{Msg, ToolDisplay};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct MessageTranscript {
    committed: Vec<String>,
    active: Option<ActiveStream>,
    tool_display: ToolDisplay,
    block: TranscriptBlock,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum TranscriptBlock {
    #[default]
    Message,
    Tools,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ActiveStream {
    message: String,
    leading_blank_line: bool,
}

impl MessageTranscript {
    pub(super) fn new(tool_display: ToolDisplay) -> Self {
        Self {
            tool_display,
            ..Self::default()
        }
    }

    pub(super) fn append(&mut self, msg: Msg, formatter: &MessageFormatter) {
        match (self.tool_display, msg) {
            (ToolDisplay::Expanded, msg) => self.committed.extend(formatter.format_msg(msg)),
            (ToolDisplay::Grouped, Msg::Tool(message)) => {
                let lines = formatter.format_tool_entry(&message);
                if !lines.is_empty() {
                    match self.block {
                        TranscriptBlock::Message => {
                            self.append_blank_line(true);
                            self.committed
                                .extend(formatter.format_message("╭─ Tool calls"));
                        }
                        TranscriptBlock::Tools => {}
                    }
                    self.committed.extend(lines);
                    self.block = TranscriptBlock::Tools;
                }
            }
            (ToolDisplay::Grouped, Msg::Empty) if self.block == TranscriptBlock::Tools => {}
            (ToolDisplay::Grouped, msg) => {
                self.finish_tool_block();
                self.committed.extend(formatter.format_msg(msg));
            }
        }
    }

    pub(super) fn pop_line(&mut self) {
        self.committed.pop();
    }

    pub(super) fn clear(&mut self) {
        self.committed.clear();
        self.active = None;
        self.block = TranscriptBlock::Message;
    }

    pub(super) fn last_line(&self) -> Option<&String> {
        self.committed.last()
    }

    pub(super) fn committed_lines(&self) -> &[String] {
        &self.committed
    }

    pub(super) fn active_lines(&self, formatter: &MessageFormatter) -> Option<Vec<String>> {
        let active = self.active.as_ref()?;
        if active.message.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        if self.needs_leading_blank_line(active.leading_blank_line) {
            lines.push(String::new());
        }
        lines.extend(formatter.format_message(&active.message));
        Some(lines)
    }

    pub(super) fn start_stream(&mut self, leading_blank_line: bool, _formatter: &MessageFormatter) {
        self.active = Some(ActiveStream {
            message: String::new(),
            leading_blank_line,
        });
    }

    pub(super) fn push_stream_chunk(&mut self, chunk: &str) {
        if !chunk.is_empty() {
            self.finish_tool_block();
        }
        self.active
            .get_or_insert_with(ActiveStream::default)
            .message
            .push_str(chunk);
    }

    pub(super) fn finish_stream(
        &mut self,
        trailing_blank_line: bool,
        formatter: &MessageFormatter,
    ) {
        let Some(active) = self.active.take() else {
            return;
        };

        if active.message.is_empty() {
            return;
        }

        self.append_blank_line(active.leading_blank_line);
        self.committed
            .extend(formatter.format_message(&active.message));

        if trailing_blank_line {
            self.append_blank_line(true);
        }
    }

    pub(super) fn take_scrollback_overflow(
        &mut self,
        live_line_capacity: usize,
        formatter: &MessageFormatter,
    ) -> Vec<String> {
        self.compact_active_stream(live_line_capacity, formatter);

        let committed_capacity =
            live_line_capacity.saturating_sub(self.active_line_count(formatter));
        let requested_flush = self.committed.len().saturating_sub(committed_capacity);
        if requested_flush == 0 {
            return Vec::new();
        }

        let flush_count =
            table_flow::flush_count_preserving_tables(&self.committed, requested_flush);
        self.drain_front(flush_count)
    }

    fn active_line_count(&self, formatter: &MessageFormatter) -> usize {
        let Some(active) = self.active.as_ref() else {
            return 0;
        };
        if active.message.is_empty() {
            return 0;
        }

        usize::from(self.needs_leading_blank_line(active.leading_blank_line))
            + formatter.format_message(&active.message).len()
    }

    fn compact_active_stream(&mut self, live_line_capacity: usize, formatter: &MessageFormatter) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.message.is_empty() {
            return;
        };
        let leading_blank_line = active.leading_blank_line;

        let leading_blank_lines = usize::from(self.needs_leading_blank_line(leading_blank_line));
        let content_capacity = live_line_capacity.saturating_sub(leading_blank_lines);
        if content_capacity == 0
            || formatter.format_message(&active.message).len() <= content_capacity
        {
            return;
        }

        let Some(split) = table_flow::split_stream_to_fit(
            &active.message,
            content_capacity,
            formatter.wrap_width(),
        ) else {
            return;
        };

        if split.prefix.is_empty() {
            return;
        }

        self.append_blank_line(leading_blank_line);
        self.committed
            .extend(formatter.format_message(&split.prefix));
        self.active = Some(ActiveStream {
            message: split.suffix,
            leading_blank_line: false,
        });
    }

    fn drain_front(&mut self, count: usize) -> Vec<String> {
        self.committed
            .drain(0..count.min(self.committed.len()))
            .collect()
    }

    fn finish_tool_block(&mut self) {
        if self.block == TranscriptBlock::Tools {
            self.committed.extend(["╰─".to_string(), String::new()]);
            self.block = TranscriptBlock::Message;
        }
    }

    fn append_blank_line(&mut self, requested: bool) {
        if requested && self.committed.last().is_some_and(|line| !line.is_empty()) {
            self.committed.push(String::new());
        }
    }

    fn needs_leading_blank_line(&self, requested: bool) -> bool {
        requested && self.committed.last().is_some_and(|line| !line.is_empty())
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/widgets/message_box/transcript/tests.rs"]
mod tests;
