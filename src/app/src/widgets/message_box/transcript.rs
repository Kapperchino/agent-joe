use super::format::MessageFormatter;
use super::table_flow;
use crate::widgets::message_box::message_box::Msg;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct MessageTranscript {
    committed: Vec<String>,
    active: Option<String>,
}

impl MessageTranscript {
    pub(super) fn append(&mut self, msg: Msg, formatter: &MessageFormatter) {
        self.committed.extend(formatter.format_msg(msg));
    }

    pub(super) fn pop_line(&mut self) {
        self.committed.pop();
    }

    pub(super) fn clear(&mut self) {
        self.committed.clear();
        self.active = None;
    }

    pub(super) fn last_line(&self) -> Option<&String> {
        self.committed.last()
    }

    pub(super) fn committed_lines(&self) -> &[String] {
        &self.committed
    }

    pub(super) fn active_lines(&self, formatter: &MessageFormatter) -> Option<Vec<String>> {
        self.active
            .as_deref()
            .map(|message| formatter.format_message(message))
    }

    pub(super) fn start_stream(&mut self, leading_blank_line: bool, formatter: &MessageFormatter) {
        if leading_blank_line {
            self.append(Msg::Empty, formatter);
        }
        self.active = Some(String::new());
    }

    pub(super) fn push_stream_chunk(&mut self, chunk: &str) {
        self.active.get_or_insert_with(String::new).push_str(chunk);
    }

    pub(super) fn finish_stream(
        &mut self,
        trailing_blank_line: bool,
        formatter: &MessageFormatter,
    ) {
        if let Some(message) = self.active.take() {
            if !message.is_empty() {
                self.committed.extend(formatter.format_message(&message));
            }
        }

        if trailing_blank_line {
            self.append(Msg::Empty, formatter);
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
        self.active
            .as_deref()
            .map(|message| formatter.format_message(message).len())
            .unwrap_or(0)
    }

    fn compact_active_stream(&mut self, live_line_capacity: usize, formatter: &MessageFormatter) {
        let Some(active_message) = self.active.as_deref() else {
            return;
        };

        if live_line_capacity == 0
            || formatter.format_message(active_message).len() <= live_line_capacity
        {
            return;
        }

        let Some(split) = table_flow::split_stream_to_fit(
            active_message,
            live_line_capacity,
            formatter.wrap_width(),
        ) else {
            return;
        };

        if split.prefix.is_empty() {
            return;
        }

        self.committed
            .extend(formatter.format_message(&split.prefix));
        self.active = Some(split.suffix);
    }

    fn drain_front(&mut self, count: usize) -> Vec<String> {
        self.committed
            .drain(0..count.min(self.committed.len()))
            .collect()
    }
}
