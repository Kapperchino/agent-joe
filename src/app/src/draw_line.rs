use markdown::mdast::Node;
use markdown::ParseOptions;
use ratatui::prelude::{Color, Line, Modifier, Span, Style};

pub struct DrawLine {}

enum Section {
    Code {
        lang: Option<String>,
        lines: Vec<String>,
    },
    Markdown(String),
}

impl DrawLine {
    pub fn render_lines(lines: &[String]) -> Vec<Line<'static>> {
        let sections = Self::split_sections(lines);
        sections
            .into_iter()
            .flat_map(|section| match section {
                Section::Code { lang, lines } => Self::render_code_section(&lang, &lines),
                Section::Markdown(text) => Self::render_markdown_section(&text),
            })
            .collect()
    }

    /// Split raw message lines into code blocks and markdown text.
    ///
    /// Code fences are detected line-by-line: an opening fence is any line
    /// starting with ``` (optionally followed by a language). A closing fence
    /// is a line consisting of *only* backticks (≥3). This matches CommonMark
    /// semantics and avoids the old toggle-on-any-backtick bug where
    /// `` ```rust `` inside a `` ```md `` block would wrongly close it.
    fn split_sections(lines: &[String]) -> Vec<Section> {
        let mut sections = Vec::new();
        let mut in_code = false;
        let mut fence_len: usize = 0;
        let mut code_lang: Option<String> = None;
        let mut code_lines: Vec<String> = Vec::new();
        let mut md_lines: Vec<String> = Vec::new();

        for line in lines {
            if in_code {
                let trimmed = line.trim();
                let backtick_count = trimmed.chars().take_while(|&c| c == '`').count();
                if backtick_count >= fence_len
                    && trimmed.len() == backtick_count
                {
                    // Closing fence: only backticks, at least as many as the opening
                    sections.push(Section::Code {
                        lang: code_lang.take(),
                        lines: std::mem::take(&mut code_lines),
                    });
                    in_code = false;
                    fence_len = 0;
                } else {
                    code_lines.push(line.clone());
                }
            } else {
                let trimmed = line.trim_start();
                let backtick_count = trimmed.chars().take_while(|&c| c == '`').count();
                if backtick_count >= 3 {
                    let rest = trimmed[backtick_count..].trim();
                    // Opening fence: backticks followed by optional info string
                    // (info string must not contain backticks per CommonMark)
                    if !rest.contains('`') {
                        if !md_lines.is_empty() {
                            sections.push(Section::Markdown(
                                std::mem::take(&mut md_lines).join("\n"),
                            ));
                        }
                        in_code = true;
                        fence_len = backtick_count;
                        code_lang = if rest.is_empty() {
                            None
                        } else {
                            Some(rest.to_string())
                        };
                        continue;
                    }
                }
                md_lines.push(line.clone());
            }
        }

        // Unclosed code block — still render it as code
        if in_code {
            sections.push(Section::Code {
                lang: code_lang,
                lines: code_lines,
            });
        }
        if !md_lines.is_empty() {
            sections.push(Section::Markdown(md_lines.join("\n")));
        }

        sections
    }

    fn render_code_section(lang: &Option<String>, lines: &[String]) -> Vec<Line<'static>> {
        let label = match lang {
            Some(l) => format!("code {}", l),
            None => "code".to_string(),
        };
        let fence_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::DIM | Modifier::BOLD);
        let code_style = Style::default().fg(Color::Yellow);

        let mut result = vec![Line::from(Span::styled(label, fence_style))];
        for line in lines {
            result.push(Line::from(Span::styled(line.clone(), code_style)));
        }
        result
    }

    fn render_markdown_section(text: &str) -> Vec<Line<'static>> {
        if text.trim().is_empty() {
            return text
                .lines()
                .map(|l| Line::from(l.to_string()))
                .collect();
        }
        let tree = match markdown::to_mdast(text, &ParseOptions::default()) {
            Ok(tree) => tree,
            Err(_) => return text.lines().map(|l| Line::from(l.to_string())).collect(),
        };
        Self::render_block(&tree)
    }

    fn render_block(node: &Node) -> Vec<Line<'static>> {
        match node {
            Node::Root(root) => root
                .children
                .iter()
                .flat_map(Self::render_block)
                .collect(),

            Node::Paragraph(para) => {
                let spans = Self::render_inline_children(&para.children, Style::default());
                Self::spans_to_lines(spans)
            }

            Node::Heading(heading) => {
                let style = match heading.depth {
                    1 => Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    2 => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    _ => Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                };
                let spans = Self::render_inline_children(&heading.children, style);
                Self::spans_to_lines(spans)
            }

            // Fallback for indented code blocks that the AST parser finds
            // (fenced blocks are already handled by split_sections)
            Node::Code(code) => {
                let label = match &code.lang {
                    Some(lang) => format!("code {}", lang),
                    None => "code".to_string(),
                };
                let fence_style = Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::DIM | Modifier::BOLD);
                let code_style = Style::default().fg(Color::Yellow);

                let mut lines = vec![Line::from(Span::styled(label, fence_style))];
                for line in code.value.lines() {
                    lines.push(Line::from(Span::styled(line.to_string(), code_style)));
                }
                lines
            }

            Node::Blockquote(bq) => {
                let inner_lines: Vec<Line> =
                    bq.children.iter().flat_map(Self::render_block).collect();
                inner_lines
                    .into_iter()
                    .map(|line| {
                        let mut spans =
                            vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
                        for span in line.spans {
                            spans.push(Span::styled(
                                span.content.into_owned(),
                                if span.style == Style::default() {
                                    Style::default().fg(Color::Gray)
                                } else {
                                    span.style
                                },
                            ));
                        }
                        Line::from(spans)
                    })
                    .collect()
            }

            Node::List(list) => {
                let mut lines = Vec::new();
                for (i, child) in list.children.iter().enumerate() {
                    if let Node::ListItem(item) = child {
                        let bullet = if list.ordered {
                            let start = list.start.unwrap_or(1);
                            format!("{}. ", start + i as u32)
                        } else {
                            "• ".to_string()
                        };
                        let bullet_style = Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD);
                        let indent = " ".repeat(bullet.chars().count());

                        let item_lines: Vec<Line> =
                            item.children.iter().flat_map(Self::render_block).collect();
                        for (j, line) in item_lines.into_iter().enumerate() {
                            let mut spans = Vec::new();
                            if j == 0 {
                                spans.push(Span::styled(bullet.clone(), bullet_style));
                            } else {
                                spans.push(Span::raw(indent.clone()));
                            }
                            spans.extend(
                                line.spans
                                    .into_iter()
                                    .map(|s| Span::styled(s.content.into_owned(), s.style)),
                            );
                            lines.push(Line::from(spans));
                        }
                    }
                }
                lines
            }

            Node::ThematicBreak(_) => {
                vec![Line::from(Span::styled(
                    "───────────────────",
                    Style::default().fg(Color::DarkGray),
                ))]
            }

            Node::Table(table) => {
                let mut all_rows: Vec<Vec<String>> = Vec::new();
                for row_node in &table.children {
                    if let Node::TableRow(tr) = row_node {
                        let cells: Vec<String> = tr
                            .children
                            .iter()
                            .map(|cell| Self::collect_text(cell))
                            .collect();
                        all_rows.push(cells);
                    }
                }

                let col_count = all_rows.iter().map(|r| r.len()).max().unwrap_or(0);
                let mut col_widths = vec![0usize; col_count];
                for row in &all_rows {
                    for (c, cell) in row.iter().enumerate() {
                        col_widths[c] = col_widths[c].max(cell.len());
                    }
                }

                let mut lines = Vec::new();
                for (r, row) in all_rows.iter().enumerate() {
                    let mut spans = Vec::new();
                    for (c, cell) in row.iter().enumerate() {
                        if c > 0 {
                            spans.push(Span::styled(
                                " │ ",
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        let padded = format!("{:<width$}", cell, width = col_widths[c]);
                        let style = if r == 0 {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        spans.push(Span::styled(padded, style));
                    }
                    lines.push(Line::from(spans));

                    if r == 0 {
                        let separator: String = col_widths
                            .iter()
                            .map(|w| "─".repeat(*w))
                            .collect::<Vec<_>>()
                            .join("─┼─");
                        lines.push(Line::from(Span::styled(
                            separator,
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                lines
            }

            Node::Html(html) => html
                .value
                .lines()
                .map(|line| {
                    Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ))
                })
                .collect(),

            Node::Math(math) => {
                let fence_style = Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::DIM);
                let math_style = Style::default().fg(Color::Magenta);
                let mut lines = vec![Line::from(Span::styled("$$", fence_style))];
                for line in math.value.lines() {
                    lines.push(Line::from(Span::styled(line.to_string(), math_style)));
                }
                lines.push(Line::from(Span::styled("$$", fence_style)));
                lines
            }

            other => {
                if let Some(children) = other.children() {
                    children.iter().flat_map(Self::render_block).collect()
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn render_inline_children(children: &[Node], base_style: Style) -> Vec<Span<'static>> {
        children
            .iter()
            .flat_map(|child| Self::render_inline(child, base_style))
            .collect()
    }

    fn render_inline(node: &Node, base_style: Style) -> Vec<Span<'static>> {
        match node {
            Node::Text(text) => {
                vec![Span::styled(text.value.clone(), base_style)]
            }

            Node::Strong(strong) => {
                let style = base_style.add_modifier(Modifier::BOLD);
                Self::render_inline_children(&strong.children, style)
            }

            Node::Emphasis(em) => {
                let style = base_style.add_modifier(Modifier::ITALIC);
                Self::render_inline_children(&em.children, style)
            }

            Node::InlineCode(code) => {
                vec![Span::styled(
                    code.value.clone(),
                    Style::default()
                        .fg(Color::Rgb(196, 167, 231))
                        .add_modifier(Modifier::BOLD),
                )]
            }

            Node::Delete(del) => {
                let style = base_style.add_modifier(Modifier::CROSSED_OUT);
                Self::render_inline_children(&del.children, style)
            }

            Node::Link(link) => {
                let link_style = base_style
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED);
                let mut spans = Self::render_inline_children(&link.children, link_style);
                spans.push(Span::styled(
                    format!(" ({})", link.url),
                    Style::default().fg(Color::DarkGray),
                ));
                spans
            }

            Node::Image(image) => {
                vec![Span::styled(
                    format!("[image: {}]", image.alt),
                    Style::default().fg(Color::Blue),
                )]
            }

            Node::InlineMath(math) => {
                vec![Span::styled(
                    math.value.clone(),
                    Style::default().fg(Color::Magenta),
                )]
            }

            Node::Html(html) => {
                vec![Span::styled(
                    html.value.clone(),
                    Style::default().fg(Color::DarkGray),
                )]
            }

            Node::Break(_) => {
                vec![Span::raw("\n")]
            }

            other => {
                if let Some(children) = other.children() {
                    Self::render_inline_children(children, base_style)
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn spans_to_lines(spans: Vec<Span<'static>>) -> Vec<Line<'static>> {
        if spans.is_empty() {
            return vec![Line::from("")];
        }

        let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
        for span in spans {
            let text = span.content.as_ref();
            if text.contains('\n') {
                let parts: Vec<&str> = text.split('\n').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        lines.push(Vec::new());
                    }
                    if !part.is_empty() {
                        lines
                            .last_mut()
                            .unwrap()
                            .push(Span::styled(part.to_string(), span.style));
                    }
                }
            } else {
                lines.last_mut().unwrap().push(span);
            }
        }

        lines.into_iter().map(Line::from).collect()
    }

    fn collect_text(node: &Node) -> String {
        match node {
            Node::Text(t) => t.value.clone(),
            Node::InlineCode(c) => c.value.clone(),
            Node::Code(c) => c.value.clone(),
            other => other
                .children()
                .map(|children| {
                    children
                        .iter()
                        .map(Self::collect_text)
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default(),
        }
    }
}