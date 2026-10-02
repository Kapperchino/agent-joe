use super::render::{LineKind, TranscriptLine};
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

    pub(super) fn format_msg(self, msg: Msg) -> Vec<TranscriptLine> {
        match msg {
            Msg::Message(message) => self
                .format_message(&message)
                .into_iter()
                .map(TranscriptLine::message)
                .collect(),
            Msg::Tool(message) => self.format_tool_message(&message),
            Msg::Empty => vec![TranscriptLine::message("")],
        }
    }

    pub(super) fn format_message(self, message: &str) -> Vec<String> {
        DrawTable::wrap_markdown_tables(message, self.wrap_width)
    }

    pub(super) fn format_tool_entry(self, summary: &str) -> Vec<TranscriptLine> {
        let indent = match self.wrap_width {
            0..=1 => "",
            _ => "│ ",
        };
        textwrap::wrap(
            summary,
            textwrap::Options::new(self.wrap_width)
                .initial_indent(indent)
                .subsequent_indent(indent),
        )
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let kind = match index {
                0 => LineKind::ToolEntry,
                _ => LineKind::ToolContinuation,
            };
            TranscriptLine::new(kind, line.into_owned())
        })
        .collect()
    }

    pub(super) fn format_tool_heading(self, heading: &str) -> Vec<TranscriptLine> {
        self.format_message(heading)
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                let kind = match index {
                    0 => LineKind::ToolHeading,
                    _ => LineKind::ToolContinuation,
                };
                TranscriptLine::new(kind, line)
            })
            .collect()
    }

    pub(super) fn tool_summary(message: &str) -> Option<String> {
        message
            .lines()
            .map(str::trim_start)
            .map(|line| {
                line.strip_prefix(TOOL_SUMMARY_PREFIX)
                    .unwrap_or(line)
                    .trim()
            })
            .find(|line| !line.is_empty())
            .map(DrawLine::expand_tabs)
    }

    fn format_tool_message(self, message: &str) -> Vec<TranscriptLine> {
        let content = message.strip_prefix(TOOL_SUMMARY_PREFIX).unwrap_or(message);
        match content.split_once('\n') {
            Some((summary, rest)) => self
                .format_tool_summary(summary)
                .into_iter()
                .chain(rest.split('\n').map(|line| {
                    TranscriptLine::new(LineKind::ToolDetail, DrawLine::expand_tabs(line))
                }))
                .collect(),
            None => self.format_tool_summary(content),
        }
    }

    fn format_tool_summary(self, summary: &str) -> Vec<TranscriptLine> {
        let summary = DrawLine::expand_tabs(summary);
        let (initial_indent, subsequent_indent) = match self.wrap_width {
            0..=1 => ("", ""),
            _ => (TOOL_SUMMARY_PREFIX, TOOL_SUMMARY_CONTINUATION_INDENT),
        };
        textwrap::wrap(
            &summary,
            textwrap::Options::new(self.wrap_width)
                .initial_indent(initial_indent)
                .subsequent_indent(subsequent_indent),
        )
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let kind = match index {
                0 => LineKind::ToolEntry,
                _ => LineKind::ToolContinuation,
            };
            TranscriptLine::new(kind, line.into_owned())
        })
        .collect()
    }
}
