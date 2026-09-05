use crate::utils::draw_line::DrawLine;
use crate::utils::draw_table::DrawTable;
use crate::widgets::message_box::message_box::Msg;

const TOOL_SUMMARY_PREFIX: &str = "- ";
const TOOL_SUMMARY_CONTINUATION_INDENT: &str = "  ";

#[derive(Debug, Clone, Copy)]
pub(super) struct MessageFormatter {
    wrap_width: usize,
}

impl MessageFormatter {
    pub(super) fn new(wrap_width: usize) -> Self {
        Self { wrap_width }
    }

    pub(super) fn wrap_width(self) -> usize {
        self.wrap_width
    }

    pub(super) fn format_msg(self, msg: Msg) -> Vec<String> {
        match msg {
            Msg::Message(message) => self.format_message(&message),
            Msg::Tool(message) => self.format_tool_message(&message),
            Msg::Empty => vec![String::new()],
        }
    }

    pub(super) fn format_message(self, message: &str) -> Vec<String> {
        DrawTable::wrap_markdown_tables(message, self.wrap_width)
    }

    fn format_tool_message(self, message: &str) -> Vec<String> {
        match message.strip_prefix(TOOL_SUMMARY_PREFIX) {
            Some(content) => match content.split_once('\n') {
                Some((summary, rest)) => self
                    .format_tool_summary(summary)
                    .into_iter()
                    .chain(rest.split('\n').map(DrawLine::expand_tabs))
                    .collect(),
                None => self.format_tool_summary(content),
            },
            None => self.format_message(message),
        }
    }

    fn format_tool_summary(self, summary: &str) -> Vec<String> {
        let summary = DrawLine::expand_tabs(summary);
        textwrap::wrap(
            &summary,
            textwrap::Options::new(self.wrap_width)
                .initial_indent(TOOL_SUMMARY_PREFIX)
                .subsequent_indent(TOOL_SUMMARY_CONTINUATION_INDENT),
        )
        .into_iter()
        .map(|line| line.into_owned())
        .collect()
    }
}
