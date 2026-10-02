use crate::theme;
use crate::utils::draw_line::{DrawLine, RenderState};
use ratatui::prelude::{Line, Modifier, Span, Style};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineKind {
    #[default]
    Message,
    ToolHeading,
    ToolEntry,
    ToolContinuation,
    ToolFooter,
    ToolDetail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TranscriptLine {
    pub(super) text: String,
    pub(super) kind: LineKind,
}

impl TranscriptLine {
    pub(super) fn new(kind: LineKind, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind,
        }
    }

    pub(super) fn message(text: impl Into<String>) -> Self {
        Self::new(LineKind::Message, text)
    }
}

#[derive(Debug, Default, Clone, Copy)]
enum InlineStyle {
    #[default]
    Text,
    Code,
}

#[derive(Debug, Default, Clone)]
pub(super) struct TranscriptRenderState {
    markdown: RenderState,
    kind: LineKind,
    inline: InlineStyle,
}

pub(super) struct TranscriptRenderer {
    draw_line: DrawLine,
    width: usize,
}

impl TranscriptRenderer {
    pub(super) fn new(width: usize) -> Self {
        Self {
            draw_line: DrawLine::new(),
            width,
        }
    }

    pub(super) fn update_width(&mut self, width: usize) {
        self.width = width;
    }

    pub(super) fn render_lines(
        &self,
        lines: &[TranscriptLine],
        state: &mut TranscriptRenderState,
    ) -> Vec<Line<'static>> {
        lines
            .chunk_by(|left, right| left.kind == right.kind)
            .flat_map(|chunk| -> Vec<Line<'static>> {
                let kind = chunk[0].kind;
                if state.kind != kind {
                    state.markdown = RenderState::default();
                }
                state.kind = kind;
                let rendered: Vec<Line<'static>> = match kind {
                    LineKind::Message | LineKind::ToolDetail => {
                        let source = chunk
                            .iter()
                            .map(|line| line.text.clone())
                            .collect::<Vec<_>>();
                        self.draw_line
                            .render_lines_with_state(&source, &mut state.markdown)
                            .into_iter()
                            .map(|line| match kind {
                                LineKind::ToolDetail => Self::detail(line),
                                _ => line,
                            })
                            .collect()
                    }
                    _ => chunk
                        .iter()
                        .map(|line| Self::tool_line(line, &mut state.inline))
                        .collect(),
                };
                rendered
                    .into_iter()
                    .map(|mut line| {
                        if kind != LineKind::Message {
                            line.spans.push(Span::styled(
                                " ".repeat(self.width.saturating_sub(line.width())),
                                Self::surface(),
                            ));
                        }
                        line
                    })
                    .collect()
            })
            .collect()
    }

    fn surface() -> Style {
        theme::base().bg(theme::SURFACE)
    }

    fn rail(text: impl Into<String>) -> Span<'static> {
        Span::styled(text.into(), Style::default().fg(theme::BORDER))
    }

    fn detail(mut line: Line<'static>) -> Line<'static> {
        line.spans.insert(0, Self::rail("│ "));
        line.style = line.style.patch(Self::surface());
        line
    }

    fn tool_line(line: &TranscriptLine, inline: &mut InlineStyle) -> Line<'static> {
        let spans = match line.kind {
            LineKind::ToolHeading => {
                let prefix = match line.text.starts_with("╭─ ") {
                    true => "╭─ ",
                    false => "",
                };
                let title = line.text.strip_prefix(prefix).unwrap_or(&line.text);
                let (label, rest) = title.split_at(title.find(" (").unwrap_or(title.len()));
                vec![
                    Self::rail(prefix),
                    Span::styled(
                        label.to_string(),
                        Style::default()
                            .fg(theme::BACKGROUND)
                            .bg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                    theme::muted(rest),
                ]
            }
            LineKind::ToolFooter => vec![Self::rail(line.text.clone())],
            LineKind::ToolEntry | LineKind::ToolContinuation => {
                let content = line
                    .text
                    .strip_prefix("│ ")
                    .or_else(|| line.text.strip_prefix("- "))
                    .or_else(|| line.text.strip_prefix("  "));
                let mut spans = content.map(|_| vec![Self::rail("│ ")]).unwrap_or_default();
                let content = content.unwrap_or(&line.text);
                let remainder = match line.kind {
                    LineKind::ToolEntry => {
                        *inline = InlineStyle::Text;
                        let (name, rest) =
                            content.split_at(content.find(' ').unwrap_or(content.len()));
                        spans.push(Span::styled(
                            name.to_string(),
                            Style::default()
                                .fg(theme::ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ));
                        rest
                    }
                    _ => content,
                };
                spans.extend(Self::arguments(remainder, inline));
                spans
            }
            _ => vec![theme::muted(line.text.clone())],
        };
        Line::from(spans).style(Self::surface())
    }

    fn arguments(content: &str, inline: &mut InlineStyle) -> Vec<Span<'static>> {
        content
            .split('`')
            .enumerate()
            .map(|(index, text)| {
                if index > 0 {
                    *inline = match inline {
                        InlineStyle::Text => InlineStyle::Code,
                        InlineStyle::Code => InlineStyle::Text,
                    };
                }
                let style = match inline {
                    InlineStyle::Text => Style::default().fg(theme::MUTED),
                    InlineStyle::Code => Style::default()
                        .fg(theme::AMBER)
                        .bg(theme::SELECTION)
                        .add_modifier(Modifier::BOLD),
                };
                Span::styled(text.to_string(), style)
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/widgets/message_box/render/tests.rs"]
mod tests;
