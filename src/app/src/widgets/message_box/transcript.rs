use super::format::MessageFormatter;
use super::table_flow;
use crate::widgets::message_box::message_box::Msg;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct MessageTranscript {
    committed: Vec<String>,
    active: Option<ActiveStream>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ActiveStream {
    message: String,
    leading_blank_line: bool,
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
mod tests {
    use super::*;

    fn formatter() -> MessageFormatter {
        MessageFormatter::new(80)
    }

    #[test]
    fn empty_stream_does_not_render_or_commit_blank_lines() {
        let formatter = formatter();
        let mut transcript = MessageTranscript::default();

        transcript.start_stream(true, &formatter);

        assert_eq!(transcript.active_lines(&formatter), None);

        transcript.finish_stream(true, &formatter);

        assert!(transcript.committed_lines().is_empty());
    }

    #[test]
    fn stream_boundaries_do_not_create_duplicate_blank_lines() {
        let formatter = formatter();
        let mut transcript = MessageTranscript::default();

        transcript.append(Msg::Message("user".to_string()), &formatter);
        transcript.start_stream(true, &formatter);
        transcript.push_stream_chunk("assistant");
        transcript.finish_stream(true, &formatter);
        transcript.start_stream(true, &formatter);
        transcript.push_stream_chunk("next");
        transcript.finish_stream(true, &formatter);

        assert_eq!(
            transcript.committed_lines(),
            &[
                "user".to_string(),
                String::new(),
                "assistant".to_string(),
                String::new(),
                "next".to_string(),
                String::new(),
            ]
        );
    }
}
