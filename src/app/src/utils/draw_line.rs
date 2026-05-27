use markdown::ParseOptions;
use markdown::mdast::{AlignKind, Node};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use textwrap::core::display_width;

const TABLE_CONTINUATION_MARKER: &str = "<!--__table_continue__-->";
const TABLE_BLOCK_CONTINUATION_MARKER: &str = "<!--__table_block_continue__-->";
const TABLE_WIDTH_MARKER_PREFIX: &str = "<!--__table_widths__:";
const HTML_COMMENT_SUFFIX: &str = "-->";

pub struct DrawLine {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderState {
    in_code: bool,
    fence_len: usize,
    code_lang: Option<String>,
}

enum Section {
    Code {
        lang: Option<String>,
        lines: Vec<String>,
    },
    Markdown(String),
}

impl DrawLine {
    pub fn new() -> Self {
        DrawLine {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    pub fn render_lines(&self, lines: &[String]) -> Vec<Line<'static>> {
        let mut state = RenderState::default();
        self.render_lines_with_state(lines, &mut state)
    }

    pub fn render_lines_with_state(
        &self,
        lines: &[String],
        state: &mut RenderState,
    ) -> Vec<Line<'static>> {
        let (sections, next_state) = Self::split_sections(lines, state.clone());
        *state = next_state;
        sections
            .into_iter()
            .flat_map(|section| match section {
                Section::Code { lang, lines } => self.render_code_section(&lang, &lines),
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
    fn split_sections(lines: &[String], mut state: RenderState) -> (Vec<Section>, RenderState) {
        let mut sections = Vec::new();
        let mut code_lines: Vec<String> = Vec::new();
        let mut markdown_lines: Vec<String> = Vec::new();

        let flush_markdown = |sections: &mut Vec<Section>, markdown_lines: &mut Vec<String>| {
            if markdown_lines.is_empty() {
                return;
            }

            sections.push(Section::Markdown(markdown_lines.join("\n")));
            markdown_lines.clear();
        };

        for line in lines {
            if state.in_code {
                let trimmed = line.trim();
                let backtick_count = trimmed.chars().take_while(|&c| c == '`').count();
                if backtick_count >= state.fence_len && trimmed.len() == backtick_count {
                    // Closing fence: only backticks, at least as many as the opening
                    sections.push(Section::Code {
                        lang: state.code_lang.take(),
                        lines: std::mem::take(&mut code_lines),
                    });
                    state.in_code = false;
                    state.fence_len = 0;
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
                        flush_markdown(&mut sections, &mut markdown_lines);
                        state.in_code = true;
                        state.fence_len = backtick_count;
                        state.code_lang = if rest.is_empty() {
                            None
                        } else {
                            Some(rest.to_string())
                        };
                        continue;
                    }
                }
                markdown_lines.push(line.clone());
            }
        }

        // Unclosed code block — still render it as code
        if state.in_code {
            sections.push(Section::Code {
                lang: state.code_lang.clone(),
                lines: code_lines,
            });
        } else {
            flush_markdown(&mut sections, &mut markdown_lines);
        }

        (sections, state)
    }

    fn render_code_section(&self, lang: &Option<String>, lines: &[String]) -> Vec<Line<'static>> {
        if lines.is_empty() {
            return vec![Line::from("")];
        }

        if lang.as_deref().is_some_and(Self::is_diff_lang) {
            return Self::render_diff_section(lines);
        }

        let mut result = Vec::new();

        let code = lines.join("\n");
        let ps = &self.syntax_set;
        let theme = &self.theme_set.themes["base16-eighties.dark"];

        let syntax = lang
            .as_deref()
            .and_then(|l| ps.find_syntax_by_token(l))
            .unwrap_or_else(|| ps.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);
        for line in LinesWithEndings::from(&code) {
            let ranges = h.highlight_line(line, ps).unwrap_or_default();
            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(
                        text.trim_end_matches('\n').to_string(),
                        Self::syntect_to_ratatui_style(style),
                    )
                })
                .collect();
            if spans.is_empty() {
                result.push(Line::from(""));
            } else {
                result.push(Line::from(spans));
            }
        }
        // Handle trailing empty lines that LinesWithEndings skips
        if code.ends_with('\n') || lines.last().is_some_and(|l| l.is_empty()) {
            if !lines.is_empty() && lines.last().is_some_and(|l| l.is_empty()) {
                result.push(Line::from(""));
            }
        }

        result
    }

    fn is_diff_lang(lang: &str) -> bool {
        lang.eq_ignore_ascii_case("diff") || lang.eq_ignore_ascii_case("patch")
    }

    fn render_diff_section(lines: &[String]) -> Vec<Line<'static>> {
        lines
            .iter()
            .map(|line| Line::from(Span::styled(line.clone(), Self::diff_line_style(line))))
            .collect()
    }

    fn diff_line_style(line: &str) -> Style {
        let trimmed = line.trim_start();
        if trimmed.starts_with('+') {
            Style::default().fg(Color::Green)
        } else if trimmed.starts_with('-') {
            Style::default().fg(Color::Red)
        } else if trimmed.starts_with("@@") {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if trimmed.starts_with("diff --git")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("rename ")
            || trimmed.starts_with("similarity ")
        {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        }
    }

    fn syntect_to_ratatui_style(style: highlighting::Style) -> Style {
        let fg = style.foreground;
        Style::default()
            .fg(Color::Rgb(fg.r, fg.g, fg.b))
            .add_modifier(
                if style.font_style.contains(highlighting::FontStyle::BOLD) {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            )
            .add_modifier(
                if style.font_style.contains(highlighting::FontStyle::ITALIC) {
                    Modifier::ITALIC
                } else {
                    Modifier::empty()
                },
            )
            .add_modifier(
                if style
                    .font_style
                    .contains(highlighting::FontStyle::UNDERLINE)
                {
                    Modifier::UNDERLINED
                } else {
                    Modifier::empty()
                },
            )
    }
    fn render_markdown_section(text: &str) -> Vec<Line<'static>> {
        if text.trim().is_empty() {
            return text
                .split('\n')
                .map(|l| Line::from(l.to_string()))
                .collect();
        }
        let normalized = Self::normalize_markdown(text);
        let tree = match markdown::to_mdast(&normalized, &Self::markdown_parse_options()) {
            Ok(tree) => tree,
            Err(_) => return text.lines().map(|l| Line::from(l.to_string())).collect(),
        };
        Self::render_block(&tree)
    }

    fn normalize_markdown(text: &str) -> String {
        text.split('\n')
            .map(Self::normalize_markdown_line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn normalize_markdown_line(line: &str) -> String {
        let indent_len = line.bytes().take_while(|b| *b == b' ').count();
        if indent_len > 3 {
            return line.to_string();
        }

        let rest = &line[indent_len..];
        let hash_count = rest.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&hash_count) {
            return line.to_string();
        }

        let after_hashes = &rest[hash_count..];
        if after_hashes.is_empty() || after_hashes.starts_with([' ', '\t']) {
            return line.to_string();
        }
        if !after_hashes
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
        {
            return line.to_string();
        }

        format!(
            "{}{} {}",
            &line[..indent_len],
            &rest[..hash_count],
            after_hashes
        )
    }

    fn is_table_block(node: &Node) -> bool {
        matches!(node, Node::Table(_))
    }

    fn render_block(node: &Node) -> Vec<Line<'static>> {
        match node {
            Node::Root(root) => {
                let mut lines = Vec::new();

                for (index, child) in root.children.iter().enumerate() {
                    if Self::is_table_block(child)
                        && index > 0
                        && !Self::is_table_block(&root.children[index - 1])
                    {
                        lines.push(Line::from(""));
                    }

                    lines.extend(Self::render_block(child));

                    if Self::is_table_block(child)
                        && index + 1 < root.children.len()
                        && !Self::is_table_block(&root.children[index + 1])
                    {
                        lines.push(Line::from(""));
                    }
                }

                lines
            }

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
                let code_style = Style::default().fg(Color::Yellow);

                let mut lines = Vec::new();
                for line in code.value.lines() {
                    lines.push(Line::from(Span::styled(line.to_string(), code_style)));
                }
                if lines.is_empty() {
                    lines.push(Line::from(""));
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

            Node::Table(table) => Self::render_table(table),

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
            Node::Html(html) if html.value == TABLE_CONTINUATION_MARKER => String::new(),
            Node::Html(html) if html.value == TABLE_BLOCK_CONTINUATION_MARKER => String::new(),
            Node::Html(html) if Self::is_table_width_marker_value(&html.value) => String::new(),
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

    fn render_table(table: &markdown::mdast::Table) -> Vec<Line<'static>> {
        let rows: Vec<&markdown::mdast::TableRow> = table
            .children
            .iter()
            .filter_map(|row| match row {
                Node::TableRow(table_row) => Some(table_row),
                _ => None,
            })
            .collect();

        if rows.is_empty() {
            return vec![Line::from("")];
        }

        let column_count = table
            .align
            .len()
            .max(rows.iter().map(|row| row.children.len()).max().unwrap_or(0));
        if column_count == 0 {
            return vec![Line::from("")];
        }

        let is_continuation = rows
            .first()
            .is_some_and(|row| Self::is_table_block_continuation_row(row));
        let column_widths = rows
            .first()
            .and_then(|row| Self::table_row_width_hint(row))
            .map(|widths| Self::normalize_table_width_hint(widths, column_count))
            .unwrap_or_else(|| {
                (0..column_count)
                    .map(|column| {
                        rows.iter()
                            .filter_map(|row| row.children.get(column))
                            .map(Self::node_display_width)
                            .max()
                            .unwrap_or(0)
                            .max(1)
                    })
                    .collect()
            });

        let mut logical_rows: Vec<Vec<&markdown::mdast::TableRow>> = Vec::new();
        for (row_index, row) in rows.iter().copied().enumerate() {
            if is_continuation && row_index == 0 {
                continue;
            }

            if Self::is_table_continuation_row(row) {
                if let Some(logical_row) = logical_rows.last_mut() {
                    logical_row.push(row);
                    continue;
                }
            }

            logical_rows.push(vec![row]);
        }

        let mut lines = Vec::with_capacity(logical_rows.len() * 2);
        if is_continuation && !logical_rows.is_empty() {
            lines.push(Self::render_table_separator(&column_widths));
        }

        for (row_index, logical_row) in logical_rows.iter().enumerate() {
            for (line_index, row) in logical_row.iter().enumerate() {
                lines.push(Self::render_table_row(
                    row,
                    &column_widths,
                    &table.align,
                    !is_continuation && row_index == 0 && line_index == 0,
                ));
            }

            if row_index < logical_rows.len().saturating_sub(1) {
                lines.push(Self::render_table_separator(&column_widths));
            }
        }

        lines
    }

    fn render_table_row(
        row: &markdown::mdast::TableRow,
        column_widths: &[usize],
        alignments: &[AlignKind],
        is_header: bool,
    ) -> Line<'static> {
        let border_style = Style::default().fg(Color::DarkGray);
        let cell_style = if is_header {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let mut spans = Vec::new();
        for (column, width) in column_widths.iter().copied().enumerate() {
            if column > 0 {
                spans.push(Span::styled(" │ ", border_style));
            }

            let align = alignments.get(column).copied().unwrap_or(AlignKind::None);
            let cell = row.children.get(column);
            let content_width = cell.map_or(0, Self::node_display_width);
            let (left_padding, right_padding) =
                Self::table_padding(width.saturating_sub(content_width), align);

            if left_padding > 0 {
                spans.push(Span::styled(" ".repeat(left_padding), cell_style));
            }

            if let Some(cell) = cell {
                spans.extend(Self::render_table_cell(cell, cell_style));
            }

            if right_padding > 0 {
                spans.push(Span::styled(" ".repeat(right_padding), cell_style));
            }
        }

        Line::from(spans)
    }

    fn render_table_cell(cell: &Node, base_style: Style) -> Vec<Span<'static>> {
        match cell {
            Node::TableCell(table_cell) => table_cell
                .children
                .iter()
                .filter(|child| !Self::is_table_hidden_marker(child))
                .flat_map(|child| Self::render_inline(child, base_style))
                .collect(),
            other => Self::render_inline(other, base_style),
        }
    }

    fn is_table_continuation_row(row: &markdown::mdast::TableRow) -> bool {
        row.children
            .first()
            .is_some_and(Self::table_cell_has_continuation_marker)
    }

    fn is_table_block_continuation_row(row: &markdown::mdast::TableRow) -> bool {
        row.children
            .first()
            .is_some_and(Self::table_cell_has_block_continuation_marker)
    }

    fn table_cell_has_continuation_marker(node: &Node) -> bool {
        match node {
            Node::TableCell(table_cell) => table_cell
                .children
                .iter()
                .any(Self::is_table_continuation_marker),
            _ => false,
        }
    }

    fn table_cell_has_block_continuation_marker(node: &Node) -> bool {
        match node {
            Node::TableCell(table_cell) => table_cell
                .children
                .iter()
                .any(Self::is_table_block_continuation_marker),
            _ => false,
        }
    }

    fn is_table_continuation_marker(node: &Node) -> bool {
        matches!(node, Node::Html(html) if html.value == TABLE_CONTINUATION_MARKER)
    }

    fn is_table_block_continuation_marker(node: &Node) -> bool {
        matches!(node, Node::Html(html) if html.value == TABLE_BLOCK_CONTINUATION_MARKER)
    }

    fn is_table_width_marker(node: &Node) -> bool {
        matches!(node, Node::Html(html) if Self::is_table_width_marker_value(&html.value))
    }

    fn is_table_hidden_marker(node: &Node) -> bool {
        Self::is_table_continuation_marker(node)
            || Self::is_table_block_continuation_marker(node)
            || Self::is_table_width_marker(node)
    }

    fn render_table_separator(column_widths: &[usize]) -> Line<'static> {
        let separator = column_widths
            .iter()
            .map(|width| "─".repeat(*width))
            .collect::<Vec<_>>()
            .join("─┼─");

        Line::from(Span::styled(
            separator,
            Style::default().fg(Color::DarkGray),
        ))
    }

    fn node_display_width(node: &Node) -> usize {
        display_width(&Self::collect_text(node))
    }

    fn table_row_width_hint(row: &markdown::mdast::TableRow) -> Option<Vec<usize>> {
        row.children.first().and_then(Self::table_cell_width_hint)
    }

    fn table_cell_width_hint(node: &Node) -> Option<Vec<usize>> {
        match node {
            Node::TableCell(table_cell) => {
                table_cell.children.iter().find_map(|child| match child {
                    Node::Html(html) => Self::parse_table_width_marker(&html.value),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    fn parse_table_width_marker(value: &str) -> Option<Vec<usize>> {
        if !Self::is_table_width_marker_value(value) {
            return None;
        }

        let payload =
            &value[TABLE_WIDTH_MARKER_PREFIX.len()..value.len() - HTML_COMMENT_SUFFIX.len()];
        let widths = payload
            .split(',')
            .map(|part| part.parse::<usize>().ok())
            .collect::<Option<Vec<_>>>()?;

        if widths.is_empty() {
            None
        } else {
            Some(widths)
        }
    }

    fn is_table_width_marker_value(value: &str) -> bool {
        value.starts_with(TABLE_WIDTH_MARKER_PREFIX) && value.ends_with(HTML_COMMENT_SUFFIX)
    }

    fn normalize_table_width_hint(mut widths: Vec<usize>, column_count: usize) -> Vec<usize> {
        widths.truncate(column_count);
        while widths.len() < column_count {
            widths.push(1);
        }
        widths.into_iter().map(|width| width.max(1)).collect()
    }

    fn table_padding(total_padding: usize, align: AlignKind) -> (usize, usize) {
        match align {
            AlignKind::Right => (total_padding, 0),
            AlignKind::Center => {
                let left = total_padding / 2;
                (left, total_padding - left)
            }
            AlignKind::Left | AlignKind::None => (0, total_padding),
        }
    }

    fn markdown_parse_options() -> ParseOptions {
        let mut options = ParseOptions::default();
        options.constructs.gfm_table = true;
        options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_diff_fence_with_add_remove_colors() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "```diff".to_string(),
            "diff --git a/file b/file".to_string(),
            "@@".to_string(),
            "-old".to_string(),
            "+new".to_string(),
            " context".to_string(),
            "```".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);

        assert_eq!(rendered[0].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(rendered[1].spans[0].style.fg, Some(Color::Cyan));
        assert_eq!(rendered[2].spans[0].style.fg, Some(Color::Red));
        assert_eq!(rendered[3].spans[0].style.fg, Some(Color::Green));
        assert_eq!(rendered[4].spans[0].style.fg, None);
    }
}
