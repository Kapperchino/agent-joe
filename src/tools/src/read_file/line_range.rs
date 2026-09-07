use crate::tool_defs::Range;
use crate::tool_error::{ToolEffects, ToolFailure, ToolFailureKind};

pub(super) struct LineRange {
    requested: Range,
}

impl TryFrom<Range> for LineRange {
    type Error = anyhow::Error;

    fn try_from(range: Range) -> anyhow::Result<Self> {
        match range.start > 0 && range.end > range.start {
            true => Ok(Self { requested: range }),
            false => Err(range_error("invalid_range", &range, None)),
        }
    }
}

impl LineRange {
    pub(super) fn render(&self, text: &str) -> anyhow::Result<String> {
        let start = self.requested.start as usize - 1;
        let lines: Vec<_> = text.lines().collect();
        let selected = lines
            .get(start..)
            .filter(|lines| !lines.is_empty())
            .ok_or_else(|| range_error("start_beyond_eof", &self.requested, Some(lines.len())))?;
        Ok(selected
            .iter()
            .take((self.requested.end - self.requested.start) as usize)
            .enumerate()
            .map(|(index, line)| format!("{}: {line}", start + index + 1))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

fn range_error(code: &str, range: &Range, line_count: Option<usize>) -> anyhow::Error {
    ToolFailure::new(ToolFailureKind::InvalidInput, ToolEffects::NoWorkspaceChange,
        serde_json::json!({ "code": code, "requested": range, "line_count": line_count,
            "message": "Lines are one-based, start is inclusive and end is exclusive. Start must exist and end must exceed start; end is clamped to EOF." }).to_string()).into()
}
