use super::{Event, Session, Snapshot};
use serde::{Deserialize, Serialize};
use tools::tool_defs::ToolResult;

pub const INLINE_BYTES: usize = 8 * 1024;
pub const ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub id: String,
    pub bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactPage {
    pub artifact: ArtifactReference,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub content: String,
}

pub struct ArtifactRange {
    offset: usize,
    bytes: usize,
}

impl ArtifactRange {
    pub fn new(offset: usize, bytes: usize) -> anyhow::Result<Self> {
        match bytes {
            1..=4096 => Ok(Self { offset, bytes }),
            _ => Err(anyhow::anyhow!("Artifact pages must contain 1–4096 bytes")),
        }
    }

    fn page(self, artifact: ArtifactReference, content: &str) -> anyhow::Result<ArtifactPage> {
        let remainder = content.get(self.offset..).ok_or_else(|| {
            anyhow::anyhow!("Artifact offset is outside the content or splits a UTF-8 character")
        })?;
        let length = remainder.floor_char_boundary(self.bytes.min(remainder.len()));
        let end = self.offset + length;
        match length == 0 && !remainder.is_empty() {
            true => Err(anyhow::anyhow!(
                "Page size cannot hold the next UTF-8 character"
            )),
            false => Ok(ArtifactPage {
                artifact,
                offset: self.offset,
                next_offset: (end < content.len()).then_some(end),
                content: remainder[..length].to_owned(),
            }),
        }
    }
}

pub fn preview(content: &str, bytes: usize) -> String {
    match content.len() > bytes {
        false => content.to_owned(),
        true => {
            let half = bytes.saturating_sub(80) / 2;
            let start = content.floor_char_boundary(half);
            let end = content.ceil_char_boundary(content.len() - half);
            format!(
                "{}\n[{} bytes omitted]\n{}",
                &content[..start],
                end - start,
                &content[end..]
            )
        }
    }
}

impl Session {
    pub fn archive_outputs(&self) -> anyhow::Result<()> {
        let mut transaction = self.store.env.write_txn()?;
        let mut snapshot = self.owned_snapshot(&transaction)?;
        let mut history = snapshot.history.clone();
        let previous_artifacts = snapshot.artifacts.len();
        for block in history.iter_mut().flat_map(|message| &mut message.content) {
            if let clients::llm::ContentBlock::ToolResult { content, .. } = block
                && content.len() > INLINE_BYTES
            {
                let artifact = self.save_artifact(&mut transaction, &mut snapshot, content)?;
                *content = artifact.preview(content);
            }
        }
        match snapshot.artifacts.len() == previous_artifacts {
            true => Ok(()),
            false => self.commit_event(transaction, snapshot, Event::OutputsArchived(history)),
        }
    }

    pub fn complete_tool(
        &self,
        operation: String,
        mut result: ToolResult,
    ) -> anyhow::Result<ToolResult> {
        let mut transaction = self.store.env.write_txn()?;
        let mut snapshot = self.owned_snapshot(&transaction)?;
        let content = match &mut result.outcome {
            Ok(content) => content,
            Err(failure) => &mut failure.message,
        };
        match serde_json::from_str::<utils::cargo::CargoResult>(content) {
            Ok(cargo) => {
                let cargo = self.archive_cargo(&mut transaction, &mut snapshot, cargo)?;
                *content = serde_json::to_string(&cargo)?;
            }
            Err(_) if content.len() > INLINE_BYTES => {
                let artifact = self.save_artifact(&mut transaction, &mut snapshot, content)?;
                *content = artifact.preview(content);
            }
            Err(_) => {}
        }
        self.commit_event(
            transaction,
            snapshot,
            Event::Completed {
                operation,
                result: result.clone(),
            },
        )?;
        Ok(result)
    }

    fn archive_cargo(
        &self,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &mut Snapshot,
        mut result: utils::cargo::CargoResult,
    ) -> anyhow::Result<utils::cargo::CargoResult> {
        for stream in [&mut result.stdout, &mut result.stderr] {
            if stream.content.len() > 1024 {
                let artifact = self.save_artifact(transaction, snapshot, &stream.content)?;
                stream.artifact = Some(artifact.into());
                stream.content = preview(&stream.content, 1024);
            }
        }
        let diagnostics = serde_json::to_string(&result.diagnostics)?;
        if diagnostics.len() > 1024 {
            let artifact = self.save_artifact(transaction, snapshot, &diagnostics)?;
            result.diagnostics_artifact = Some(artifact.into());
            result.diagnostics.clear();
        }
        Ok(result)
    }

    pub(crate) fn complete_process(&self, result: utils::cargo::CargoResult) -> anyhow::Result<()> {
        let mut transaction = self.store.env.write_txn()?;
        let mut snapshot = self.owned_snapshot(&transaction)?;
        let result = self.archive_cargo(&mut transaction, &mut snapshot, result)?;
        self.commit_event(
            transaction,
            snapshot,
            Event::ProcessCompleted(Box::new(result)),
        )
    }

    pub(super) fn save_artifact(
        &self,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &mut Snapshot,
        content: &str,
    ) -> anyhow::Result<ArtifactReference> {
        let artifact = ArtifactReference::new(self.store.storage.new_id(), content.len())?;
        self.store
            .artifacts
            .put(transaction, &artifact.id, content.as_bytes())?;
        self.store
            .artifact_index
            .record(transaction, self.store.snapshots, snapshot, &artifact)?;
        snapshot.artifacts.push(artifact.clone());
        Ok(artifact)
    }

    pub fn read_artifact(&self, id: &str, range: ArtifactRange) -> anyhow::Result<ArtifactPage> {
        let transaction = self.store.env.read_txn()?;
        let artifact = match self.store.owner(&transaction, &self.id)? {
            Some(owner) if owner == self.owner => {
                self.store.artifact_index.get(&transaction, &self.id, id)
            }
            _ => Err(anyhow::anyhow!("Session {} ownership was lost", self.id)),
        }?;
        let bytes = self
            .store
            .artifacts
            .get(&transaction, id)?
            .ok_or_else(|| anyhow::anyhow!("Artifact {id} is missing"))?;
        range.page(artifact, std::str::from_utf8(bytes)?)
    }
}

impl ArtifactReference {
    fn new(id: String, bytes: usize) -> anyhow::Result<Self> {
        match bytes <= ARTIFACT_BYTES {
            true => Ok(Self { id, bytes }),
            false => Err(anyhow::anyhow!("Output exceeds the 64 MiB artifact limit")),
        }
    }

    fn preview(&self, content: &str) -> String {
        format!(
            "{}\n[Full output: artifact {} ({} bytes). Use read_artifact with offset 0 and bytes 4096; follow next_offset for more.]",
            preview(content, INLINE_BYTES - 512),
            self.id,
            self.bytes,
        )
    }
}

impl From<ArtifactReference> for utils::cargo::OutputArtifact {
    fn from(artifact: ArtifactReference) -> Self {
        Self {
            id: artifact.id,
            bytes: artifact.bytes,
        }
    }
}
