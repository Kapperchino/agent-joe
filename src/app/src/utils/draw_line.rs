use markdown::ParseOptions;
use markdown::mdast::{AlignKind, Node};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{self, ThemeSet};
use syntect::parsing::SyntaxSet;
use textwrap::core::display_width;

const TABLE_CONTINUATION_MARKER: &str = "<!--__table_continue__-->";
const TABLE_BLOCK_CONTINUATION_MARKER: &str = "<!--__table_block_continue__-->";
const TABLE_WIDTH_MARKER_PREFIX: &str = "<!--__table_widths__:";
const HTML_COMMENT_SUFFIX: &str = "-->";
const CODE_THEME: &str = "base16-eighties.dark";
const TAB_STOP_WIDTH: usize = 4;

pub struct DrawLine {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderState {
    fence: Option<CodeFence>,
}

enum Section {
    Code(CodeSection),
    Markdown(String),
}

struct CodeSection {
    lang: Option<String>,
    lines: Vec<String>,
    trim_leading_blank_lines: bool,
    trim_trailing_blank_lines: bool,
}

struct ListMarker<'a> {
    indent: usize,
    content_indent: usize,
    content: &'a str,
}

impl Default for DrawLine {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeSection {
    fn new(
        lang: Option<String>,
        lines: Vec<String>,
        trim_leading_blank_lines: bool,
        trim_trailing_blank_lines: bool,
    ) -> Self {
        Self {
            lang,
            lines,
            trim_leading_blank_lines,
            trim_trailing_blank_lines,
        }
    }

    fn trimmed_lines(&self) -> &[String] {
        let mut start = 0;
        let mut end = self.lines.len();

        if self.trim_leading_blank_lines {
            while start < end && self.lines[start].trim().is_empty() {
                start += 1;
            }
        }

        if self.trim_trailing_blank_lines {
            while end > start && self.lines[end - 1].trim().is_empty() {
                end -= 1;
            }
        }

        &self.lines[start..end]
    }
}

struct SectionSplitter {
    state: RenderState,
    sections: Vec<Section>,
    markdown_lines: Vec<String>,
    code_lines: Vec<String>,
    code_started_in_current_batch: bool,
}

impl SectionSplitter {
    fn split(lines: &[String], state: RenderState) -> (Vec<Section>, RenderState) {
        let mut splitter = Self {
            state,
            sections: Vec::new(),
            markdown_lines: Vec::new(),
            code_lines: Vec::new(),
            code_started_in_current_batch: false,
        };

        for line in lines {
            splitter.push_line(line);
        }

        splitter.finish()
    }

    fn push_line(&mut self, line: &str) {
        if self.state.fence.is_some() {
            self.push_code_line(line);
        } else {
            self.push_markdown_line(line);
        }
    }

    fn push_code_line(&mut self, line: &str) {
        if self
            .state
            .fence
            .as_ref()
            .is_some_and(|fence| fence.is_closing(line))
        {
            self.flush_code(true);
            self.close_code();
        } else if self.should_recover_markdown(line) {
            self.flush_code(false);
            self.close_code();
            self.push_markdown_line(line);
        } else {
            self.code_lines.push(line.to_string());
        }
    }

    fn push_markdown_line(&mut self, line: &str) {
        match CodeFence::opening(line) {
            Some(fence) => {
                self.flush_markdown();
                self.open_code(fence);
            }
            None => self.markdown_lines.push(line.to_string()),
        }
    }

    fn finish(mut self) -> (Vec<Section>, RenderState) {
        if self.state.fence.is_some() {
            self.flush_code(false);
        } else {
            self.flush_markdown();
        }

        (self.sections, self.state)
    }

    fn flush_markdown(&mut self) {
        if !self.markdown_lines.is_empty() {
            self.sections
                .push(Section::Markdown(self.markdown_lines.join("\n")));
            self.markdown_lines.clear();
        }
    }

    fn flush_code(&mut self, trim_trailing_blank_lines: bool) {
        if !self.code_lines.is_empty() || self.code_started_in_current_batch {
            let lang = self
                .state
                .fence
                .as_ref()
                .and_then(|fence| fence.lang.clone());
            self.sections.push(Section::Code(CodeSection::new(
                lang,
                std::mem::take(&mut self.code_lines),
                self.code_started_in_current_batch,
                trim_trailing_blank_lines,
            )));
        }
    }

    fn open_code(&mut self, fence: CodeFence) {
        self.state.fence = Some(fence);
        self.code_started_in_current_batch = true;
    }

    fn close_code(&mut self) {
        self.state.fence = None;
        self.code_started_in_current_batch = false;
    }

    fn should_recover_markdown(&self, line: &str) -> bool {
        !self
            .state
            .fence
            .as_ref()
            .and_then(|fence| fence.lang.as_deref())
            .is_some_and(DrawLine::is_diff_lang)
            && self
                .code_lines
                .last()
                .is_some_and(|line| line.trim().is_empty())
            && self.looks_like_markdown_after_code(line)
    }

    fn looks_like_markdown_after_code(&self, line: &str) -> bool {
        DrawLine::markdown_list_marker(line).is_some_and(|marker| {
            let content = marker.content.trim_start();
            self.code_content_is_elided()
                || content.starts_with("**")
                || content.starts_with("__")
                || content.starts_with('`')
                || content.starts_with('[')
                || content.contains("**")
                || content.contains("__")
        })
    }

    fn code_content_is_elided(&self) -> bool {
        let mut content = self
            .code_lines
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .peekable();
        content.peek().is_some() && content.all(|line| matches!(line, "..." | "…"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeFence {
    marker: char,
    len: usize,
    lang: Option<String>,
}

impl CodeFence {
    pub(crate) fn opening(line: &str) -> Option<Self> {
        let trimmed = line.trim_start();
        match trimmed.chars().next() {
            Some(marker @ ('`' | '~')) => {
                let len = Self::leading_markers(trimmed, marker);
                let info = trimmed[len..].trim();
                (len >= 3 && !(marker == '`' && info.contains('`'))).then(|| Self {
                    marker,
                    len,
                    lang: Self::language(info),
                })
            }
            _ => None,
        }
    }

    pub(crate) fn is_closing(&self, line: &str) -> bool {
        let trimmed = line.trim();
        let len = Self::leading_markers(trimmed, self.marker);
        len >= self.len && trimmed.len() == len
    }

    fn leading_markers(line: &str, marker: char) -> usize {
        line.chars().take_while(|&ch| ch == marker).count()
    }

    fn language(info: &str) -> Option<String> {
        let token = info.split_whitespace().next()?;
        let token = token
            .trim_matches(|ch| matches!(ch, '{' | '}' | '.'))
            .strip_prefix("language-")
            .unwrap_or_else(|| token.trim_matches(|ch| matches!(ch, '{' | '}' | '.')));
        let token = token
            .split(|ch| matches!(ch, ',' | ':' | ';'))
            .next()
            .unwrap_or(token)
            .trim_matches(|ch| matches!(ch, '{' | '}' | '.'));

        (!token.is_empty()).then(|| token.to_string())
    }
}

impl DrawLine {
    pub fn new() -> Self {
        Self {
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
                Section::Code(section) => self.render_code_section(&section),
                Section::Markdown(text) => Self::render_markdown_section(&text),
            })
            .collect()
    }
    fn split_sections(lines: &[String], state: RenderState) -> (Vec<Section>, RenderState) {
        SectionSplitter::split(lines, state)
    }

    fn render_code_section(&self, section: &CodeSection) -> Vec<Line<'static>> {
        let lines = section.trimmed_lines();
        if lines.is_empty() {
            vec![Line::from("")]
        } else if section.lang.as_deref().is_some_and(Self::is_diff_lang) {
            Self::render_diff_section(lines)
        } else {
            let ps = &self.syntax_set;
            let theme = &self.theme_set.themes[CODE_THEME];
            let syntax = section
                .lang
                .as_deref()
                .and_then(|lang| ps.find_syntax_by_token(lang))
                .unwrap_or_else(|| ps.find_syntax_plain_text());
            let mut highlighter = HighlightLines::new(syntax, theme);
            lines
                .iter()
                .map(|line| self.render_highlighted_code_line(line, &mut highlighter))
                .collect()
        }
    }

    fn is_diff_lang(lang: &str) -> bool {
        lang.eq_ignore_ascii_case("diff") || lang.eq_ignore_ascii_case("patch")
    }

    fn render_diff_section(lines: &[String]) -> Vec<Line<'static>> {
        lines
            .iter()
            .map(|line| {
                let line = Self::expand_tabs(line);
                let style = Self::diff_line_style(&line);
                Line::from(Span::styled(line, style))
            })
            .collect()
    }

    fn render_highlighted_code_line(
        &self,
        line: &str,
        highlighter: &mut HighlightLines<'_>,
    ) -> Line<'static> {
        let line = Self::expand_tabs(line);
        match highlighter.highlight_line(&line, &self.syntax_set) {
            Ok(ranges) => {
                let spans = ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), Self::syntect_to_ratatui_style(style))
                    })
                    .collect::<Vec<_>>();

                if spans.is_empty() {
                    Line::from("")
                } else {
                    Line::from(spans)
                }
            }
            Err(_) => Line::from(line.to_string()),
        }
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
            text.split('\n')
                .map(|line| Line::from(Self::expand_tabs(line)))
                .collect()
        } else {
            let normalized = Self::normalize_markdown(text);
            match markdown::to_mdast(&normalized, &Self::markdown_parse_options()) {
                Ok(Node::Root(root)) => Self::render_root(&root, &normalized),
                Ok(tree) => Self::render_block(&tree),
                Err(_) => text
                    .lines()
                    .map(|line| Line::from(Self::expand_tabs(line)))
                    .collect(),
            }
        }
    }

    fn render_root(root: &markdown::mdast::Root, source: &str) -> Vec<Line<'static>> {
        let source_lines = source.split('\n').collect::<Vec<_>>();
        let mut lines = Vec::new();

        for (index, child) in root.children.iter().enumerate() {
            if Self::is_table_block(child)
                && index > 0
                && !Self::is_table_block(&root.children[index - 1])
            {
                lines.push(Line::from(""));
            }

            let rendered = match child {
                Node::List(list) => Self::render_list(list, Some(&source_lines)),
                _ => {
                    Self::restore_root_indentation(child, Self::render_block(child), &source_lines)
                }
            };
            lines.extend(rendered);

            if Self::is_table_block(child)
                && index + 1 < root.children.len()
                && !Self::is_table_block(&root.children[index + 1])
            {
                lines.push(Line::from(""));
            }
        }

        lines
    }

    fn restore_root_indentation(
        node: &Node,
        mut rendered: Vec<Line<'static>>,
        source_lines: &[&str],
    ) -> Vec<Line<'static>> {
        if let Some(position) = node.position() {
            let start = position.start.line.saturating_sub(1);
            if let Some(first_source_line) = source_lines.get(start) {
                let end = if position.end.column == 1 {
                    position.end.line.saturating_sub(1)
                } else {
                    position.end.line
                };
                let source_block = source_lines.get(start..end).unwrap_or_default();
                if matches!(node, Node::Paragraph(_)) && source_block.len() == rendered.len() {
                    for (line, source_line) in rendered.iter_mut().zip(source_block) {
                        Self::prepend_indent(line, Self::leading_space_count(source_line));
                    }
                } else {
                    let indent = if matches!(node, Node::Code(_)) {
                        Self::leading_space_count(first_source_line).min(TAB_STOP_WIDTH)
                    } else {
                        Self::leading_space_count(first_source_line)
                    };
                    for line in &mut rendered {
                        Self::prepend_indent(line, indent);
                    }
                }
            }
        }
        rendered
    }

    fn prepend_indent(line: &mut Line<'static>, indent: usize) {
        if indent > 0 && !Self::line_plain_text(line).is_empty() {
            line.spans.insert(0, Span::raw(" ".repeat(indent)));
        }
    }

    fn leading_space_count(line: &str) -> usize {
        line.bytes().take_while(|byte| *byte == b' ').count()
    }

    pub(crate) fn expand_tabs(text: &str) -> String {
        if text.contains('\t') {
            let mut expanded = String::with_capacity(text.len());
            let mut column = 0;

            for ch in text.chars() {
                match ch {
                    '\t' => {
                        let spaces = TAB_STOP_WIDTH - (column % TAB_STOP_WIDTH);
                        expanded.push_str(&" ".repeat(spaces));
                        column += spaces;
                    }
                    '\n' | '\r' => {
                        expanded.push(ch);
                        column = 0;
                    }
                    _ => {
                        let mut buffer = [0; 4];
                        column += display_width(ch.encode_utf8(&mut buffer));
                        expanded.push(ch);
                    }
                }
            }

            expanded
        } else {
            text.to_string()
        }
    }

    fn normalize_markdown(text: &str) -> String {
        let mut normalized = Vec::new();
        let mut previous_line: Option<String> = None;
        let mut loose_nested_content_indent: Option<usize> = None;

        for mut line in text.split('\n').map(Self::normalize_markdown_line) {
            if let (Some(content_indent), Some(marker)) = (
                loose_nested_content_indent,
                Self::markdown_list_marker(&line),
            ) {
                if marker.indent > 0 && marker.indent < content_indent {
                    let extra_indent = " ".repeat(content_indent - marker.indent);
                    line = format!("{extra_indent}{line}");
                }
            }

            if Self::is_list_marker_line(&line)
                && previous_line
                    .as_deref()
                    .is_some_and(Self::list_should_interrupt_after)
            {
                normalized.push(String::new());
            }

            loose_nested_content_indent =
                Self::next_loose_nested_content_indent(&line, loose_nested_content_indent);
            previous_line = Some(line.clone());
            normalized.push(line);
        }

        normalized.join("\n")
    }

    fn next_loose_nested_content_indent(line: &str, current: Option<usize>) -> Option<usize> {
        match Self::markdown_list_marker(line) {
            Some(marker) if marker.content.trim_end().ends_with(':') => Some(marker.content_indent),
            Some(marker) if marker.indent > 0 => current,
            None if line.trim().is_empty() => current,
            _ => None,
        }
    }

    fn normalize_markdown_line(line: &str) -> String {
        let line = Self::expand_tabs(line);
        let indent_len = Self::leading_space_count(&line);
        let rest = &line[indent_len..];
        let hash_count = rest.bytes().take_while(|byte| *byte == b'#').count();
        let after_hashes = &rest[hash_count..];
        if indent_len <= 3
            && (1..=6).contains(&hash_count)
            && after_hashes
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_digit())
        {
            format!(
                "{}{} {}",
                &line[..indent_len],
                &rest[..hash_count],
                after_hashes
            )
        } else {
            line
        }
    }

    fn list_should_interrupt_after(line: &str) -> bool {
        let trimmed = line.trim();
        !trimmed.is_empty() && !Self::is_list_marker_line(line)
    }

    fn is_list_marker_line(line: &str) -> bool {
        Self::markdown_list_content_indent(line).is_some()
    }

    pub(crate) fn markdown_list_content_indent(line: &str) -> Option<String> {
        Self::markdown_list_marker(line).map(|marker| " ".repeat(marker.content_indent))
    }

    pub(crate) fn markdown_list_initial_indent(line: &str) -> Option<String> {
        Self::markdown_list_marker(line).map(|marker| " ".repeat(marker.indent))
    }

    fn markdown_list_marker(line: &str) -> Option<ListMarker<'_>> {
        let indent_len = line.bytes().take_while(|b| *b == b' ').count();
        let rest = &line[indent_len..];
        let rest_bytes = rest.as_bytes();
        match rest_bytes.first().copied() {
            Some(b'-' | b'*' | b'+') => {
                let marker_len = 1 + Self::following_space_len(&rest_bytes[1..])?;
                Some(ListMarker {
                    indent: indent_len,
                    content_indent: indent_len + marker_len,
                    content: &rest[marker_len..],
                })
            }
            Some(b'0'..=b'9') => {
                let digit_len = rest_bytes
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                match rest_bytes.get(digit_len).copied() {
                    Some(b'.' | b')') if (1..=9).contains(&digit_len) => {
                        let marker_len = digit_len
                            + 1
                            + Self::following_space_len(&rest_bytes[digit_len + 1..])?;
                        Some(ListMarker {
                            indent: indent_len,
                            content_indent: indent_len + marker_len,
                            content: &rest[marker_len..],
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn following_space_len(bytes: &[u8]) -> Option<usize> {
        let len = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        (len > 0).then_some(len)
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
            Node::Code(code) => {
                let code_style = Style::default();

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

            Node::List(list) => Self::render_list(list, None),

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

    fn render_list(
        list: &markdown::mdast::List,
        source_lines: Option<&[&str]>,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (i, child) in list.children.iter().enumerate() {
            let Node::ListItem(item) = child else {
                continue;
            };
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
            let source_indent = source_lines
                .and_then(|lines| {
                    item.position.as_ref().and_then(|position| {
                        lines
                            .get(position.start.line.saturating_sub(1))
                            .map(|line| Self::leading_space_count(line))
                    })
                })
                .unwrap_or_default();

            let item_lines: Vec<Line> = item.children.iter().flat_map(Self::render_block).collect();
            for (j, line) in item_lines.into_iter().enumerate() {
                let mut spans = Vec::new();
                if source_indent > 0 {
                    spans.push(Span::raw(" ".repeat(source_indent)));
                }
                if j == 0 {
                    spans.push(Span::styled(bullet.clone(), bullet_style));
                } else if let Some(nested_spans) =
                    Self::render_loose_nested_list_line(&line, &indent, bullet_style)
                {
                    spans.extend(nested_spans);
                    lines.push(Line::from(spans));
                    continue;
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
        lines
    }

    fn render_loose_nested_list_line(
        line: &Line<'static>,
        parent_indent: &str,
        bullet_style: Style,
    ) -> Option<Vec<Span<'static>>> {
        let plain_text = Self::line_plain_text(line);
        Self::markdown_list_marker(&plain_text)
            .filter(|marker| !marker.content.trim().is_empty())
            .map(|marker| {
                let mut spans = vec![
                    Span::raw(parent_indent.to_string()),
                    Span::styled("• ", bullet_style),
                ];
                spans.extend(Self::strip_span_prefix(
                    line.spans.clone(),
                    marker.content_indent,
                ));
                spans
            })
    }

    fn line_plain_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn strip_span_prefix(spans: Vec<Span<'static>>, mut prefix_len: usize) -> Vec<Span<'static>> {
        let mut stripped = Vec::new();

        for span in spans {
            let content = span.content.into_owned();
            if prefix_len >= content.len() {
                prefix_len -= content.len();
                continue;
            }

            let content = content[prefix_len..].to_string();
            prefix_len = 0;
            if !content.is_empty() {
                stripped.push(Span::styled(content, span.style));
            }
        }

        stripped
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
            Node::Link(link) => format!(
                "{} ({})",
                link.children
                    .iter()
                    .map(Self::collect_text)
                    .collect::<String>(),
                link.url
            ),
            Node::Image(image) => format!("[image: {}]", image.alt),
            Node::InlineMath(math) => math.value.clone(),
            Node::Break(_) => "\n".to_string(),
            Node::Html(html) if html.value == TABLE_CONTINUATION_MARKER => String::new(),
            Node::Html(html) if html.value == TABLE_BLOCK_CONTINUATION_MARKER => String::new(),
            Node::Html(html) if Self::is_table_width_marker_value(&html.value) => String::new(),
            Node::Html(html) => html.value.clone(),
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

        let column_count = table
            .align
            .len()
            .max(rows.iter().map(|row| row.children.len()).max().unwrap_or(0));
        if column_count == 0 {
            vec![Line::from("")]
        } else {
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
        Self::collect_text(node)
            .split('\n')
            .map(display_width)
            .max()
            .unwrap_or_default()
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
        let payload = value
            .strip_prefix(TABLE_WIDTH_MARKER_PREFIX)?
            .strip_suffix(HTML_COMMENT_SUFFIX)?;
        payload
            .split(',')
            .map(|part| part.parse::<usize>().ok())
            .collect::<Option<Vec<_>>>()
            .filter(|widths| !widths.is_empty())
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
#[path = "../../tests/unit/utils/draw_line/tests.rs"]
mod tests;
