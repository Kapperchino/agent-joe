use crate::tool_defs::Range;

pub(super) struct LineRange {
    lines: std::ops::Range<usize>,
}

impl TryFrom<Range> for LineRange {
    type Error = anyhow::Error;

    fn try_from(range: Range) -> anyhow::Result<Self> {
        if range.start == 0 || range.end <= range.start {
            Err(anyhow::anyhow!(
                "Line ranges must start at one or later and end after the start"
            ))
        } else {
            Ok(Self {
                lines: range.start as usize - 1..range.end as usize - 1,
            })
        }
    }
}

impl LineRange {
    pub(super) fn render(&self, text: &str) -> anyhow::Result<String> {
        let lines: Vec<_> = text.lines().collect();
        let selected = lines
            .get(self.lines.start..)
            .filter(|lines| !lines.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("The requested start line is beyond the end of the file")
            })?;
        Ok(selected
            .iter()
            .take(self.lines.len())
            .enumerate()
            .map(|(index, line)| format!("{}: {line}", self.lines.start + index + 1))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}
