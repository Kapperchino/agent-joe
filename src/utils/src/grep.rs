use futures::{StreamExt, TryStreamExt};
use grep::regex::RegexMatcher;
use grep::searcher::{Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use std::fmt;
use std::path::PathBuf;

pub struct Grep {}

#[derive(Debug, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: String,
    pub lines: Vec<GrepLine>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GrepLine {
    pub line_number: Option<u64>,
    pub line: String,
}

impl fmt::Display for GrepLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line_number {
            Some(i) => write!(f, "{i}: {}", self.line),
            None => f.write_str(&self.line),
        }
    }
}

impl fmt::Display for GrepMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.path)?;
        for line in &self.lines {
            write!(f, "\n{line}")?;
        }
        Ok(())
    }
}

struct LineCollector {
    groups: Vec<Vec<GrepLine>>,
    bytes: usize,
}

impl LineCollector {
    fn new() -> Self {
        Self {
            groups: Vec::new(),
            bytes: 0,
        }
    }

    fn current_group(&mut self) -> &mut Vec<GrepLine> {
        if self.groups.is_empty() {
            self.groups.push(Vec::new());
        }
        self.groups.last_mut().unwrap()
    }

    fn push(&mut self, line_number: Option<u64>, bytes: &[u8]) -> std::io::Result<()> {
        self.bytes = self.bytes.saturating_add(bytes.len()).saturating_add(32);
        match self.bytes <= 32 * 1024 * 1024 {
            true => Ok(()),
            false => Err(std::io::Error::other(
                "Search output exceeds 32 MiB; narrow the pattern or context",
            )),
        }?;
        self.current_group().push(GrepLine {
            line_number,
            line: String::from_utf8_lossy(bytes)
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        });
        Ok(())
    }
}

impl Sink for LineCollector {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.push(mat.line_number(), mat.bytes())?;
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        ctx: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        self.push(ctx.line_number(), ctx.bytes())?;
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        if self.groups.last().is_some_and(|group| !group.is_empty()) {
            self.groups.push(Vec::new());
        }
        Ok(true)
    }
}

impl Grep {
    pub async fn grep(
        regex: &str,
        files: Vec<PathBuf>,
        before: usize,
        after: usize,
    ) -> anyhow::Result<Vec<GrepMatch>> {
        let context = SearchContext::new(before, after)?;
        let scope = crate::execution::ExecutionScope::current();
        let result = futures::stream::iter(files)
            .map(|file| {
                let scope = scope.clone();
                let regex = regex.to_owned();
                async move {
                    scope
                        .enter(async move {
                            let content = crate::files::Files::read_file(&file).await?;
                            let matcher = RegexMatcher::new(&regex)?;
                            let mut searcher = SearcherBuilder::new()
                                .line_number(true)
                                .before_context(context.before)
                                .after_context(context.after)
                                .build();
                            let mut collector = LineCollector::new();
                            searcher.search_slice(&matcher, content.as_bytes(), &mut collector)?;
                            Ok::<_, anyhow::Error>(
                                collector
                                    .groups
                                    .into_iter()
                                    .filter(|group| !group.is_empty())
                                    .map(|lines| GrepMatch {
                                        path: file.to_string_lossy().into_owned(),
                                        lines,
                                    })
                                    .collect(),
                            )
                        })
                        .await
                }
            })
            .buffered(4)
            .try_fold(
                SearchResults::default(),
                |results, group: Vec<GrepMatch>| async move { results.append(group) },
            )
            .await?;
        Ok(result.groups)
    }
}

#[derive(Clone, Copy)]
struct SearchContext {
    before: usize,
    after: usize,
}

impl SearchContext {
    fn new(before: usize, after: usize) -> anyhow::Result<Self> {
        match before <= 1000 && after <= 1000 {
            true => Ok(Self { before, after }),
            false => Err(anyhow::anyhow!(
                "Search context is limited to 1000 lines before and after a match"
            )),
        }
    }
}

#[derive(Default)]
struct SearchResults {
    groups: Vec<GrepMatch>,
    bytes: usize,
}

impl SearchResults {
    fn append(mut self, group: Vec<GrepMatch>) -> anyhow::Result<Self> {
        self.bytes = group.iter().fold(self.bytes, |bytes, group| {
            bytes
                .saturating_add(group.to_string().len())
                .saturating_add(2)
        });
        match self.bytes <= 32 * 1024 * 1024 {
            true => {
                self.groups.extend(group);
                Ok(self)
            }
            false => Err(anyhow::anyhow!(
                "Search output exceeds 32 MiB; narrow the pattern or context"
            )),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/grep/tests.rs"]
mod tests;
