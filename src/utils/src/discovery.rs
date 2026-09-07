use crate::{
    inventory::{Inventory, ResultLimit},
    workspace::WorkspacePolicy,
};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub enum SearchMode {
    Literal,
    Regex,
}

#[derive(Clone, Copy)]
pub enum SearchTarget {
    Paths,
    Text,
}

pub struct SearchQuery {
    pattern: Regex,
    include: GlobSet,
    exclude: GlobSet,
    limit: ResultLimit,
    before: usize,
    after: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    pub truncation: Option<TruncationReason>,
    pub scanned_files: usize,
    pub skipped_files: usize,
    pub limit: usize,
    pub skipped: Vec<SkippedFile>,
    pub skipped_details_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationReason {
    ResultLimit,
    OutputLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub lines: Vec<SearchLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchLine {
    pub line: usize,
    pub text: String,
}

impl SearchQuery {
    pub fn new(
        pattern: &str,
        mode: SearchMode,
        include: &str,
        exclude: &str,
        limit: Option<usize>,
        before: usize,
        after: usize,
    ) -> anyhow::Result<Self> {
        match before <= 1000 && after <= 1000 {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Search context is limited to 1000 lines on each side"
            )),
        }?;
        let pattern = match mode {
            SearchMode::Literal => Regex::new(&regex::escape(pattern)),
            SearchMode::Regex => Regex::new(pattern),
        }?;
        Ok(Self {
            pattern,
            include: globs(include)?,
            exclude: globs(exclude)?,
            limit: ResultLimit::new(limit)?,
            before,
            after,
        })
    }

    pub fn search(
        &self,
        workspace: &WorkspacePolicy,
        inventory: Inventory,
        target: SearchTarget,
    ) -> anyhow::Result<SearchResult> {
        let mut result = SearchResult {
            matches: Vec::new(),
            truncated: false,
            truncation: None,
            scanned_files: 0,
            skipped_files: inventory.skipped,
            limit: self.limit.get(),
            skipped: Vec::new(),
            skipped_details_truncated: false,
        };
        let mut bytes = 0usize;
        let mut files = inventory.files.into_iter().filter(|path| {
            (self.include.is_empty() || self.include.is_match(path)) && !self.exclude.is_match(path)
        });
        while !result.truncated
            && let Some(path) = files.next()
        {
            result.scanned_files += 1;
            match target {
                SearchTarget::Paths if self.pattern.is_match(&path.to_string_lossy()) => {
                    self.append(
                        &mut result,
                        &mut bytes,
                        SearchMatch {
                            path,
                            line: None,
                            lines: Vec::new(),
                        },
                    )?;
                }
                SearchTarget::Paths => {}
                SearchTarget::Text => match workspace.read(&path) {
                    Ok(content) if !content.contains('\0') => {
                        let lines = content.lines().collect::<Vec<_>>();
                        let mut found = lines
                            .iter()
                            .enumerate()
                            .filter(|(_, line)| self.pattern.is_match(line));
                        while !result.truncated
                            && let Some((index, _)) = found.next()
                        {
                            let start = index.saturating_sub(self.before);
                            let end = index
                                .saturating_add(self.after)
                                .saturating_add(1)
                                .min(lines.len());
                            let lines = lines[start..end]
                                .iter()
                                .enumerate()
                                .map(|(offset, line)| SearchLine {
                                    line: start + offset + 1,
                                    text: (*line).to_owned(),
                                })
                                .collect();
                            self.append(
                                &mut result,
                                &mut bytes,
                                SearchMatch {
                                    path: path.clone(),
                                    line: Some(index + 1),
                                    lines,
                                },
                            )?;
                        }
                    }
                    skipped => {
                        result.skipped_files += 1;
                        let reason = match skipped {
                            Ok(_) => "Binary content".into(),
                            Err(error) => error.to_string(),
                        };
                        match result.skipped.len() < 100 {
                            true => result.skipped.push(SkippedFile {
                                path,
                                reason: reason.chars().take(2048).collect(),
                            }),
                            false => result.skipped_details_truncated = true,
                        }
                    }
                },
            }
        }
        match serde_json::to_vec(&result)?.len() <= 32 * 1024 * 1024 {
            true => Ok(result),
            false => Err(anyhow::anyhow!(
                "Search output and metadata exceed 32 MiB; narrow the pattern or filters"
            )),
        }
    }

    fn append(
        &self,
        result: &mut SearchResult,
        bytes: &mut usize,
        found: SearchMatch,
    ) -> anyhow::Result<()> {
        let size = serde_json::to_vec(&found)?.len();
        result.truncation = match () {
            _ if result.matches.len() >= self.limit.get() => Some(TruncationReason::ResultLimit),
            _ if bytes.saturating_add(size) > 31 * 1024 * 1024 => {
                Some(TruncationReason::OutputLimit)
            }
            _ => {
                *bytes += size;
                result.matches.push(found);
                None
            }
        };
        result.truncated = result.truncation.is_some();
        Ok(())
    }
}

fn globs(patterns: &str) -> anyhow::Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns.lines().filter(|line| !line.is_empty()) {
        builder.add(GlobBuilder::new(pattern).literal_separator(true).build()?);
    }
    Ok(builder.build()?)
}
