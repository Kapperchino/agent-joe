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
    in_code: bool,
    fence_marker: Option<char>,
    fence_len: usize,
    code_lang: Option<String>,
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
        if self.state.in_code {
            self.push_code_line(line);
        } else {
            self.push_markdown_line(line);
        }
    }

    fn push_code_line(&mut self, line: &str) {
        if self
            .state
            .fence_marker
            .is_some_and(|marker| CodeFence::is_closing(line, marker, self.state.fence_len))
        {
            self.flush_code(true, true);
            self.close_code();
            return;
        }

        if self.should_recover_markdown(line) {
            self.flush_code(true, false);
            self.close_code();
            self.push_markdown_line(line);
            return;
        }

        self.code_lines.push(line.to_string());
    }

    fn push_markdown_line(&mut self, line: &str) {
        let Some(fence) = CodeFence::opening(line) else {
            self.markdown_lines.push(line.to_string());
            return;
        };

        self.flush_markdown();
        self.open_code(fence);
    }

    fn finish(mut self) -> (Vec<Section>, RenderState) {
        if self.state.in_code {
            self.flush_code(false, false);
        } else {
            self.flush_markdown();
        }

        (self.sections, self.state)
    }

    fn flush_markdown(&mut self) {
        if self.markdown_lines.is_empty() {
            return;
        }

        self.sections
            .push(Section::Markdown(self.markdown_lines.join("\n")));
        self.markdown_lines.clear();
    }

    fn flush_code(&mut self, close_fence: bool, trim_trailing_blank_lines: bool) {
        if self.code_lines.is_empty() && !self.code_started_in_current_batch {
            if close_fence {
                self.state.code_lang.take();
            }
            return;
        }

        let lang = if close_fence {
            self.state.code_lang.take()
        } else {
            self.state.code_lang.clone()
        };

        self.sections.push(Section::Code(CodeSection::new(
            lang,
            std::mem::take(&mut self.code_lines),
            self.code_started_in_current_batch,
            trim_trailing_blank_lines,
        )));
    }

    fn open_code(&mut self, fence: CodeFence) {
        self.state.in_code = true;
        self.state.fence_marker = Some(fence.marker);
        self.state.fence_len = fence.len;
        self.state.code_lang = fence.lang;
        self.code_started_in_current_batch = true;
    }

    fn close_code(&mut self) {
        self.state.in_code = false;
        self.state.fence_marker = None;
        self.state.fence_len = 0;
        self.code_started_in_current_batch = false;
    }

    fn should_recover_markdown(&self, line: &str) -> bool {
        if self
            .state
            .code_lang
            .as_deref()
            .is_some_and(DrawLine::is_diff_lang)
        {
            return false;
        }

        self.code_lines
            .last()
            .is_some_and(|line| line.trim().is_empty())
            && self.looks_like_markdown_after_code(line)
    }

    fn looks_like_markdown_after_code(&self, line: &str) -> bool {
        let Some(marker) = DrawLine::markdown_list_marker(line) else {
            return false;
        };

        let content = marker.content.trim_start();
        self.code_content_is_elided()
            || content.starts_with("**")
            || content.starts_with("__")
            || content.starts_with('`')
            || content.starts_with('[')
            || content.contains("**")
            || content.contains("__")
    }

    fn code_content_is_elided(&self) -> bool {
        let mut saw_content = false;

        for line in &self.code_lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            saw_content = true;
            if !matches!(trimmed, "..." | "…") {
                return false;
            }
        }

        saw_content
    }
}

struct CodeFence {
    marker: char,
    len: usize,
    lang: Option<String>,
}

impl CodeFence {
    fn opening(line: &str) -> Option<Self> {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next()?;
        if !matches!(marker, '`' | '~') {
            return None;
        }

        let len = Self::leading_markers(trimmed, marker);
        if len < 3 {
            return None;
        }

        let info = trimmed[len..].trim();
        if marker == '`' && info.contains('`') {
            return None;
        }

        Some(Self {
            marker,
            len,
            lang: Self::language(info),
        })
    }

    fn is_closing(line: &str, marker: char, opening_len: usize) -> bool {
        let trimmed = line.trim();
        let len = Self::leading_markers(trimmed, marker);
        len >= opening_len && trimmed.len() == len
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

    /// Split raw message lines into code blocks and markdown text.
    ///
    /// Code fences are detected line-by-line: an opening fence is any line
    /// starting with at least three backticks or tildes (optionally followed by
    /// a language). A closing fence uses the same marker and consists only of
    /// at least as many markers as the opening fence. This avoids toggling on
    /// marker-like content inside a code block.
    fn split_sections(lines: &[String], state: RenderState) -> (Vec<Section>, RenderState) {
        SectionSplitter::split(lines, state)
    }

    fn render_code_section(&self, section: &CodeSection) -> Vec<Line<'static>> {
        let lines = section.trimmed_lines();
        if lines.is_empty() {
            return vec![Line::from("")];
        }

        if section.lang.as_deref().is_some_and(Self::is_diff_lang) {
            return Self::render_diff_section(lines);
        }

        let ps = &self.syntax_set;
        let theme = &self.theme_set.themes[CODE_THEME];
        let syntax = section
            .lang
            .as_deref()
            .and_then(|l| ps.find_syntax_by_token(l))
            .unwrap_or_else(|| ps.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);
        lines
            .iter()
            .map(|line| self.render_highlighted_code_line(line, &mut h))
            .collect()
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
            return text
                .split('\n')
                .map(|line| Line::from(Self::expand_tabs(line)))
                .collect();
        }
        let normalized = Self::normalize_markdown(text);
        let tree = match markdown::to_mdast(&normalized, &Self::markdown_parse_options()) {
            Ok(tree) => tree,
            Err(_) => {
                return text
                    .lines()
                    .map(|line| Line::from(Self::expand_tabs(line)))
                    .collect();
            }
        };
        match &tree {
            Node::Root(root) => Self::render_root(root, &normalized),
            _ => Self::render_block(&tree),
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
                _ => Self::restore_root_indentation(
                    child,
                    Self::render_block(child),
                    &source_lines,
                ),
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
        let Some(position) = node.position() else {
            return rendered;
        };
        let start = position.start.line.saturating_sub(1);
        let Some(first_source_line) = source_lines.get(start) else {
            return rendered;
        };

        if matches!(node, Node::Paragraph(_)) {
            let end = if position.end.column == 1 {
                position.end.line.saturating_sub(1)
            } else {
                position.end.line
            };
            let source_block = source_lines.get(start..end).unwrap_or_default();
            if source_block.len() == rendered.len() {
                for (line, source_line) in rendered.iter_mut().zip(source_block) {
                    Self::prepend_indent(line, Self::leading_space_count(source_line));
                }
                return rendered;
            }
        }

        let indent = if matches!(node, Node::Code(_)) {
            Self::leading_space_count(first_source_line).min(TAB_STOP_WIDTH)
        } else {
            Self::leading_space_count(first_source_line)
        };
        for line in &mut rendered {
            Self::prepend_indent(line, indent);
        }
        rendered
    }

    fn prepend_indent(line: &mut Line<'static>, indent: usize) {
        if indent == 0 || Self::line_plain_text(line).is_empty() {
            return;
        }
        line.spans.insert(0, Span::raw(" ".repeat(indent)));
    }

    fn leading_space_count(line: &str) -> usize {
        line.bytes().take_while(|byte| *byte == b' ').count()
    }

    pub(crate) fn expand_tabs(text: &str) -> String {
        if !text.contains('\t') {
            return text.to_string();
        }

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
        let Some(marker) = Self::markdown_list_marker(line) else {
            return if line.trim().is_empty() {
                current
            } else {
                None
            };
        };

        if marker.content.trim_end().ends_with(':') {
            return Some(marker.content_indent);
        }

        if marker.indent == 0 { None } else { current }
    }

    fn normalize_markdown_line(line: &str) -> String {
        let line = Self::expand_tabs(line);
        let indent_len = line.bytes().take_while(|b| *b == b' ').count();
        if indent_len > 3 {
            return line;
        }

        let rest = &line[indent_len..];
        let hash_count = rest.bytes().take_while(|b| *b == b'#').count();
        if !(1..=6).contains(&hash_count) {
            return line;
        }

        let after_hashes = &rest[hash_count..];
        if after_hashes.is_empty() || after_hashes.starts_with([' ', '\t']) {
            return line;
        }
        if !after_hashes
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
        {
            return line;
        }

        format!(
            "{}{} {}",
            &line[..indent_len],
            &rest[..hash_count],
            after_hashes
        )
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
                if digit_len == 0 || digit_len > 9 {
                    return None;
                }

                match rest_bytes.get(digit_len).copied() {
                    Some(b'.' | b')') => {
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

            // Fallback for indented code blocks that the AST parser finds
            // (fenced blocks are already handled by split_sections)
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

            Node::List(list) => {
                Self::render_list(list, None)
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
                .and_then(|lines| item.position.as_ref().and_then(|position| {
                    lines
                        .get(position.start.line.saturating_sub(1))
                        .map(|line| Self::leading_space_count(line))
                }))
                .unwrap_or_default();

            let item_lines: Vec<Line> =
                item.children.iter().flat_map(Self::render_block).collect();
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
        let plain_text = Self::line_plain_text(&line);
        let marker = Self::markdown_list_marker(&plain_text)?;
        if marker.content.trim().is_empty() {
            return None;
        }

        let mut spans = vec![
            Span::raw(parent_indent.to_string()),
            Span::styled("• ", bullet_style),
        ];
        spans.extend(Self::strip_span_prefix(
            line.spans.clone(),
            marker.content_indent,
        ));
        Some(spans)
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
    use crate::utils::draw_table::DrawTable;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

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

    #[test]
    fn extracts_language_from_fence_info_string() {
        assert_eq!(
            CodeFence::language("rust src/lib.rs").as_deref(),
            Some("rust")
        );
        assert_eq!(CodeFence::language("rust,ignore").as_deref(), Some("rust"));
        assert_eq!(CodeFence::language("{.rust}").as_deref(), Some("rust"));
        assert_eq!(
            CodeFence::language("language-rust").as_deref(),
            Some("rust")
        );
    }

    #[test]
    fn renders_diff_fence_with_extra_info_string() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "```diff changes.patch".to_string(),
            "-old".to_string(),
            "+new".to_string(),
            "```".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);

        assert_eq!(rendered[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(rendered[1].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn indented_markdown_code_does_not_force_yellow_text() {
        let draw_line = DrawLine::new();
        let lines = vec!["    parser fallback".to_string()];

        let rendered = draw_line.render_lines(&lines);

        assert_eq!(rendered[0].spans[0].style.fg, None);
        assert_eq!(line_text(&rendered[0]), "    parser fallback");
    }

    #[test]
    fn preserves_root_indentation_when_rendering_partial_markdown() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "  wrapped list continuation".to_string(),
            "   - orphaned nested item".to_string(),
            "- next root item".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "  wrapped list continuation".to_string(),
                "   • orphaned nested item".to_string(),
                "• next root item".to_string(),
            ]
        );
    }

    #[test]
    fn trims_padding_between_heading_and_fenced_code_content() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "### take_scrollback_overflow".to_string(),
            "```rust".to_string(),
            String::new(),
            String::new(),
            "pub(super) fn take_scrollback_overflow(".to_string(),
            "```".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "take_scrollback_overflow".to_string(),
                "pub(super) fn take_scrollback_overflow(".to_string(),
            ]
        );
    }

    #[test]
    fn preserves_blank_lines_inside_fenced_code_content() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "```rust".to_string(),
            "fn one() {}".to_string(),
            String::new(),
            "fn two() {}".to_string(),
            "```".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "fn one() {}".to_string(),
                String::new(),
                "fn two() {}".to_string(),
            ]
        );
    }

    #[test]
    fn keeps_code_fence_state_across_render_batches() {
        let draw_line = DrawLine::new();
        let mut state = RenderState::default();

        let first = draw_line
            .render_lines_with_state(&["```diff".to_string(), "-old".to_string()], &mut state);
        let second = draw_line.render_lines_with_state(
            &["+new".to_string(), "```".to_string(), "after".to_string()],
            &mut state,
        );

        assert_eq!(first[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(second[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(line_text(&second[1]), "after");
        assert!(!state.in_code);
    }

    #[test]
    fn preserves_blank_code_prefix_when_fence_started_in_previous_batch() {
        let draw_line = DrawLine::new();
        let mut state = RenderState::default();

        draw_line.render_lines_with_state(&["```rust".to_string()], &mut state);
        let rendered = draw_line.render_lines_with_state(
            &[String::new(), "fn main() {}".to_string(), "```".to_string()],
            &mut state,
        );
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text, vec![String::new(), "fn main() {}".to_string()]);
        assert!(!state.in_code);
    }

    #[test]
    fn language_fence_inside_code_content_does_not_close_block() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "```md".to_string(),
            "```rust".to_string(),
            "fn main() {}".to_string(),
            "```".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec!["```rust".to_string(), "fn main() {}".to_string()]
        );
    }

    #[test]
    fn renders_tilde_fences_and_requires_matching_closing_marker() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "~~~rust".to_string(),
            "fn main() {}".to_string(),
            "```".to_string(),
            "~~~".to_string(),
            "after".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "fn main() {}".to_string(),
                "```".to_string(),
                "after".to_string(),
            ]
        );
    }

    #[test]
    fn recovers_markdown_list_after_elided_unclosed_code_fence() {
        let draw_line = DrawLine::new();
        let mut state = RenderState::default();
        let lines = vec![
            "```rust".to_string(),
            "    ...".to_string(),
            String::new(),
            String::new(),
            "- **Render diffs specially**".to_string(),
            "  - Detects diff-like languages.".to_string(),
        ];

        let rendered = draw_line.render_lines_with_state(&lines, &mut state);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "    ...".to_string(),
                String::new(),
                String::new(),
                "• Render diffs specially".to_string(),
                "  • Detects diff-like languages.".to_string(),
            ]
        );
        assert!(!state.in_code);
    }

    #[test]
    fn diff_fences_do_not_recover_on_removed_markdown_like_lines() {
        let draw_line = DrawLine::new();
        let mut state = RenderState::default();
        let lines = vec![
            "```diff".to_string(),
            " context".to_string(),
            String::new(),
            "- **removed heading**".to_string(),
        ];

        let rendered = draw_line.render_lines_with_state(&lines, &mut state);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                " context".to_string(),
                String::new(),
                "- **removed heading**".to_string(),
            ]
        );
        assert_eq!(rendered[2].spans[0].style.fg, Some(Color::Red));
        assert!(state.in_code);
    }

    #[test]
    fn renders_wrapped_markdown_lists_without_raw_markers_or_flush_left_continuations() {
        let draw_line = DrawLine::new();
        let wrapped = DrawTable::wrap_markdown_tables(
            "splits it into:\n- prefix: committed to history\n- suffix: remains active\n- Uses table_flow::split_stream_to_fit, preserving table context when splitting markdown tables.",
            68,
        );

        let rendered = draw_line.render_lines(&wrapped);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text[0], "splits it into:");
        assert_eq!(text[1], "• prefix: committed to history");
        assert_eq!(text[2], "• suffix: remains active");
        assert!(text.iter().all(|line| !line.starts_with("- ")));
        assert!(
            text.iter()
                .any(|line| line.starts_with("  when splitting markdown tables."))
        );
    }

    #[test]
    fn renders_loose_nested_list_continuations_as_nested_bullets() {
        let draw_line = DrawLine::new();
        let lines = vec![
            "- DrawLine".to_string(),
            "- Holds:".to_string(),
            " - `SyntaxSet` from `syntect` for syntax lookup.".to_string(),
            " - `ThemeSet` from `syntect` for code highlighting.".to_string(),
            "- Created with `DrawLine::new()`.".to_string(),
        ];

        let rendered = draw_line.render_lines(&lines);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            text,
            vec![
                "• DrawLine".to_string(),
                "• Holds:".to_string(),
                "  • SyntaxSet from syntect for syntax lookup.".to_string(),
                "  • ThemeSet from syntect for code highlighting.".to_string(),
                "• Created with DrawLine::new().".to_string(),
            ]
        );
    }

    #[test]
    fn wrapping_preserves_nested_list_initial_indent() {
        let wrapped = DrawTable::wrap_markdown_tables(
            "  - `SyntaxSet` from `syntect` for syntax lookup and another long phrase",
            36,
        );

        assert!(wrapped[0].starts_with("  - "));
        assert!(wrapped[1].starts_with("    "));
    }

    #[test]
    fn wraps_long_table_headers_to_the_viewport_width() {
        let draw_line = DrawLine::new();
        let wrapped = DrawTable::wrap_markdown_tables(
            "| This table header is much too long |\n| --- |\n| value |",
            12,
        );
        let rendered = draw_line.render_lines(&wrapped);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();
        let separator_index = text
            .iter()
            .position(|line| !line.is_empty() && line.chars().all(|ch| ch == '─'))
            .expect("table separator should be rendered");

        assert!(separator_index > 1, "header should wrap: {text:?}");
        assert!(
            text.iter().all(|line| display_width(line) <= 12),
            "rendered table overflowed: {text:?}"
        );
    }

    #[test]
    fn expands_tabs_at_four_column_stops() {
        assert_eq!(DrawLine::expand_tabs("\talpha"), "    alpha");
        assert_eq!(DrawLine::expand_tabs("ab\talpha"), "ab  alpha");
        assert_eq!(DrawLine::expand_tabs("abc\talpha"), "abc alpha");
    }

    #[test]
    fn tab_indented_nested_lists_wrap_and_render_consistently() {
        let draw_line = DrawLine::new();
        let wrapped = DrawTable::wrap_markdown_tables(
            "- parent\n\t- child content that wraps onto another line",
            30,
        );

        assert!(wrapped.iter().all(|line| !line.contains('\t')));
        assert!(wrapped[1].starts_with("    - "));
        assert!(wrapped[2].starts_with("      "));

        let rendered = draw_line.render_lines(&wrapped);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text[0], "• parent");
        assert!(text[1].starts_with("  • child content"));
        assert!(text[2].starts_with("    "));
    }

    #[test]
    fn expands_tabs_in_fenced_code_without_wrapping_code_lines() {
        let draw_line = DrawLine::new();
        let wrapped = DrawTable::wrap_markdown_tables(
            "```rust\n\tlet value = 1;\nvalue\t+= 1;\n```",
            12,
        );
        let rendered = draw_line.render_lines(&wrapped);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text, vec!["    let value = 1;", "value   += 1;"]);
    }
}
