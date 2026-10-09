use super::{FileVersion, content_diff};
use crate::workspace::FILE_READ_LIMIT;
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FileSnapshot {
    Content(FileVersion),
    Fingerprint {
        digest: String,
        mode: u32,
        size: u64,
    },
}

impl FileSnapshot {
    pub(crate) fn read(mut file: File, mode: u32) -> anyhow::Result<Self> {
        let mut content = Vec::new();
        file.by_ref()
            .take(FILE_READ_LIMIT as u64 + 1)
            .read_to_end(&mut content)?;
        match content.len() <= FILE_READ_LIMIT {
            true => Ok(Self::Content(FileVersion::File { content, mode })),
            false => {
                let mut hasher = blake3::Hasher::new();
                hasher.update(&content);
                let size = content.len() as u64 + std::io::copy(&mut file, &mut hasher)?;
                Ok(Self::Fingerprint {
                    digest: hasher.finalize().to_string(),
                    mode,
                    size,
                })
            }
        }
    }

    pub fn content(&self) -> Option<&FileVersion> {
        match self {
            Self::Content(version) => Some(version),
            Self::Fingerprint { .. } => None,
        }
    }

    pub fn fingerprint(&self) -> String {
        match self {
            Self::Content(version) => version.fingerprint(),
            Self::Fingerprint { digest, mode, .. } => format!("{mode:o}:{digest}"),
        }
    }

    pub fn diff(&self, path: &Path, after: &Self) -> String {
        match (self.content(), after.content()) {
            (Some(before), Some(after)) => content_diff(path, before, after),
            _ => format!(
                "File {}: {} -> {} (content omitted above 16 MiB)\n",
                path.display(),
                self.fingerprint(),
                after.fingerprint()
            ),
        }
    }
}
