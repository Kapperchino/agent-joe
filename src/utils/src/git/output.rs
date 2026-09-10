use super::{GitResult, OUTPUT_LIMIT, excluded};
use git2::{DiffFormat, DiffOptions};

impl GitResult {
    pub(super) fn bounded(self) -> anyhow::Result<Self> {
        match serde_json::to_vec(&self)?.len() <= OUTPUT_LIMIT {
            true => Ok(self),
            false => Err(anyhow::anyhow!(
                "Git result exceeds the 32 MiB output limit"
            )),
        }
    }
}

pub(super) struct BlobContent {
    pub text: String,
}

impl BlobContent {
    pub fn new(repo: &git2::Repository, id: git2::Oid) -> anyhow::Result<Self> {
        let size = repo.odb()?.read_header(id)?.0;
        match size <= OUTPUT_LIMIT {
            true => {
                let blob = repo.find_blob(id)?;
                Self::from_bytes(blob.content())
            }
            false => Err(anyhow::anyhow!("Git blob exceeds the 32 MiB output limit")),
        }
    }

    fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        match bytes.len() <= OUTPUT_LIMIT {
            true => {
                let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
                    anyhow::anyhow!("The Git blob is binary; diff reports binary change metadata")
                })?;
                Ok(Self { text })
            }
            false => Err(anyhow::anyhow!(
                "Git content exceeds the 32 MiB output limit"
            )),
        }
    }
}

enum DiffOutput {
    Collecting { bytes: Vec<u8> },
    Exceeded,
}

impl DiffOutput {
    fn append(&mut self, line: &git2::DiffLine<'_>) -> bool {
        let prefix = matches!(line.origin(), '+' | '-' | ' ');
        let length = line.content().len().saturating_add(usize::from(prefix));
        match self {
            Self::Collecting { bytes } if bytes.len().saturating_add(length) <= OUTPUT_LIMIT => {
                bytes.extend(
                    prefix
                        .then_some(line.origin() as u8)
                        .into_iter()
                        .chain(line.content().iter().copied()),
                );
                true
            }
            Self::Collecting { .. } | Self::Exceeded => {
                *self = Self::Exceeded;
                false
            }
        }
    }

    fn finish(self, result: Result<(), git2::Error>) -> anyhow::Result<String> {
        match self {
            Self::Collecting { bytes } => {
                result?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            }
            Self::Exceeded => Err(anyhow::anyhow!("Git diff exceeds the 32 MiB output limit")),
        }
    }
}

pub(super) fn render_diff(diff: &git2::Diff<'_>) -> anyhow::Result<String> {
    let mut output = DiffOutput::Collecting { bytes: Vec::new() };
    let result = diff.print(DiffFormat::Patch, |delta, _, line| {
        let hidden = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .is_some_and(excluded);
        match hidden {
            true => true,
            false => output.append(&line),
        }
    });
    output.finish(result)
}

pub(super) fn diff_options() -> DiffOptions {
    let mut options = DiffOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true)
        .disable_pathspec_match(true)
        .ignore_submodules(true)
        .skip_binary_check(false);
    options
}
