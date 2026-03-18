use ratatui::prelude::{Color, Line, Modifier, Span, Style};

pub struct DrawLine {}

impl DrawLine {
    pub fn render_lines(lines: &[String]) -> Vec<Line<'static>> {
        let mut in_code_block = false;
        lines
            .iter()
            .map(|message| Self::render_line(message, &mut in_code_block))
            .collect()
    }
    fn render_line(message: &str, in_code_block: &mut bool) -> Line<'static> {
        if message.starts_with("--- [tool:") && message.ends_with("] ---") {
            return Line::from(Span::styled(
                message.to_string(),
                Style::default().fg(Color::Cyan),
            ));
        }

        if let Some(language) = message.strip_prefix("```") {
            *in_code_block = !*in_code_block;
            let fence_label = if language.is_empty() {
                "code".to_string()
            } else {
                format!("code {}", language)
            };
            return Line::from(Span::styled(
                fence_label,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            ));
        }

        if *in_code_block {
            return Line::from(Span::styled(
                message.to_string(),
                Style::default().fg(Color::Yellow),
            ));
        }

        if let Some(content) = message.strip_prefix("# ") {
            return Line::from(Self::inline_markdown_spans(
                content,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if let Some(content) = message.strip_prefix("## ") {
            return Line::from(Self::inline_markdown_spans(
                content,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if let Some(content) = message.strip_prefix("### ") {
            return Line::from(Self::inline_markdown_spans(
                content,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if let Some(content) = message.strip_prefix("#### ") {
            return Line::from(Self::inline_markdown_spans(
                content,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if let Some(content) = message.strip_prefix("> ") {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
            spans.extend(Self::inline_markdown_spans(
                content,
                Style::default().fg(Color::Gray),
            ));
            return Line::from(spans);
        }

        if let Some(content) = message
            .strip_prefix("- ")
            .or_else(|| message.strip_prefix("* "))
            .or_else(|| message.strip_prefix("+ "))
        {
            let mut spans = vec![Span::styled(
                "• ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )];
            spans.extend(Self::inline_markdown_spans(content, Style::default()));
            return Line::from(spans);
        }

        if let Some((prefix, content)) = Self::ordered_list_parts(message) {
            let mut spans = vec![Span::styled(
                prefix.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )];
            spans.extend(Self::inline_markdown_spans(content, Style::default()));
            return Line::from(spans);
        }

        Line::from(Self::inline_markdown_spans(message, Style::default()))
    }

    fn inline_markdown_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        let mut segment_start = 0;
        let mut index = 0;
        let mut bold = false;
        let mut inline_code = false;

        while index < text.len() {
            let rest = &text[index..];

            if !inline_code && rest.starts_with("**") {
                Self::push_markdown_span(
                    &mut spans,
                    &text[segment_start..index],
                    base_style,
                    bold,
                    inline_code,
                );
                bold = !bold;
                index += 2;
                segment_start = index;
                continue;
            }

            if rest.starts_with('`') {
                Self::push_markdown_span(
                    &mut spans,
                    &text[segment_start..index],
                    base_style,
                    bold,
                    inline_code,
                );
                inline_code = !inline_code;
                index += 1;
                segment_start = index;
                continue;
            }

            index += rest
                .chars()
                .next()
                .map_or(1, std::primitive::char::len_utf8);
        }

        Self::push_markdown_span(
            &mut spans,
            &text[segment_start..],
            base_style,
            bold,
            inline_code,
        );

        if spans.is_empty() {
            spans.push(Span::styled(text.to_string(), base_style));
        }

        spans
    }

    fn ordered_list_parts(message: &str) -> Option<(&str, &str)> {
        let marker_end = message.find(". ")?;
        let (number, rest) = message.split_at(marker_end);
        if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }

        Some((&message[..marker_end + 2], rest[2..].trim_start()))
    }

    fn push_markdown_span(
        spans: &mut Vec<Span<'static>>,
        text: &str,
        base_style: Style,
        bold: bool,
        inline_code: bool,
    ) {
        if text.is_empty() {
            return;
        }

        let style = if inline_code {
            Style::default()
                .fg(Color::Rgb(196, 167, 231))
                .add_modifier(Modifier::BOLD)
        } else if bold {
            base_style.add_modifier(Modifier::BOLD)
        } else {
            base_style
        };

        spans.push(Span::styled(text.to_string(), style));
    }
}
