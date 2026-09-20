use super::{Event, Session, SessionDatabase, Snapshot};
use serde::{Deserialize, Serialize};
use tools::tool_defs::ToolResult;

pub const INLINE_BYTES: usize = 8 * 1024;
pub const ARTIFACT_PAGE_BYTES: usize = 32 * 1024;
pub use utils::artifacts::{ARTIFACT_BYTES, ArtifactReference};

#[derive(Clone, Copy, Default)]
enum OutputLimit {
    #[default]
    Tool,
    Review,
    ArtifactPage,
}

impl OutputLimit {
    fn for_tool(name: &str) -> Self {
        match name {
            "review_changes" => Self::Review,
            "read_artifact" => Self::ArtifactPage,
            _ => Self::Tool,
        }
    }

    fn bytes(self) -> usize {
        match self {
            Self::Tool => INLINE_BYTES,
            Self::Review => ARTIFACT_PAGE_BYTES,
            Self::ArtifactPage => ARTIFACT_PAGE_BYTES + 512,
        }
    }
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
            1..=ARTIFACT_PAGE_BYTES => Ok(Self { offset, bytes }),
            _ => Err(anyhow::anyhow!(
                "Artifact pages must contain 1–{ARTIFACT_PAGE_BYTES} bytes"
            )),
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

pub use utils::text::preview;

impl Session {
    pub fn archive_outputs(&self) -> anyhow::Result<()> {
        self.store.update(Some(&self.id), |database| {
            let mut transaction = database.env.write_txn()?;
            let mut snapshot = self.owned_snapshot(database, &transaction)?;
            let mut history = snapshot.history.clone();
            let limits = snapshot
                .history
                .iter()
                .flat_map(|message| &message.content)
                .filter_map(|block| match block {
                    clients::llm::ContentBlock::ToolBlock { tool_id, name, .. } => {
                        Some((tool_id.id.to_string(), OutputLimit::for_tool(name.as_ref())))
                    }
                    _ => None,
                })
                .collect::<std::collections::BTreeMap<_, _>>();
            let previous_artifacts = snapshot.artifacts.len();
            history
                .iter_mut()
                .flat_map(|message| &mut message.content)
                .try_for_each(|block| match block {
                    clients::llm::ContentBlock::ToolResult {
                        tool_id, content, ..
                    } if content.len()
                        > limits
                            .get(tool_id.id.as_ref())
                            .copied()
                            .unwrap_or_default()
                            .bytes() =>
                    {
                        let artifact =
                            self.save_artifact(database, &mut transaction, &mut snapshot, content)?;
                        *content = artifact_preview(&artifact, content);
                        Ok::<_, anyhow::Error>(())
                    }
                    _ => Ok(()),
                })?;
            match snapshot.artifacts.len() == previous_artifacts {
                true => Ok(()),
                false => self.commit_event(
                    database,
                    transaction,
                    snapshot,
                    Event::OutputsArchived(history),
                ),
            }
        })
    }

    pub fn complete_tool(
        &self,
        operation: String,
        result: ToolResult,
    ) -> anyhow::Result<ToolResult> {
        self.store.update(Some(&self.id), |database| {
            let mut result = result.clone();
            let mut transaction = database.env.write_txn()?;
            let mut snapshot = self.owned_snapshot(database, &transaction)?;
            let limit = OutputLimit::for_tool(result.invocation.name.as_ref()).bytes();
            let content = match &mut result.outcome {
                Ok(content) => content,
                Err(failure) => &mut failure.message,
            };
            match serde_json::from_str::<utils::cargo::CargoResult>(content) {
                Ok(cargo) => {
                    let cargo =
                        self.archive_cargo(database, &mut transaction, &mut snapshot, cargo)?;
                    *content = serde_json::to_string(&cargo)?;
                }
                Err(_) if content.len() > limit => {
                    let artifact =
                        self.save_artifact(database, &mut transaction, &mut snapshot, content)?;
                    *content = artifact_preview(&artifact, content);
                }
                Err(_) => {}
            }
            self.commit_event(
                database,
                transaction,
                snapshot,
                Event::Completed {
                    operation: operation.clone(),
                    result: result.clone(),
                },
            )?;
            Ok(result)
        })
    }

    fn archive_cargo(
        &self,
        database: &SessionDatabase,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &mut Snapshot,
        mut result: utils::cargo::CargoResult,
    ) -> anyhow::Result<utils::cargo::CargoResult> {
        for stream in [&mut result.stdout, &mut result.stderr] {
            if stream.content.len() > 1024 {
                let artifact =
                    self.save_artifact(database, transaction, snapshot, &stream.content)?;
                stream.artifact = Some(artifact);
                stream.content = preview(&stream.content, 1024);
            }
        }
        let diagnostics = serde_json::to_string(&result.diagnostics)?;
        if diagnostics.len() > 1024 {
            let artifact = self.save_artifact(database, transaction, snapshot, &diagnostics)?;
            result.diagnostics_artifact = Some(artifact);
            result.diagnostics.clear();
        }
        Ok(result)
    }

    pub fn complete_process(&self, result: utils::cargo::CargoResult) -> anyhow::Result<()> {
        self.store.update(Some(&self.id), |database| {
            let result = result.clone();
            let mut transaction = database.env.write_txn()?;
            let mut snapshot = self.owned_snapshot(database, &transaction)?;
            let result = self.archive_cargo(database, &mut transaction, &mut snapshot, result)?;
            self.commit_event(
                database,
                transaction,
                snapshot,
                Event::ProcessCompleted(Box::new(result)),
            )
        })
    }

    pub(super) fn save_artifact(
        &self,
        database: &SessionDatabase,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &mut Snapshot,
        content: &str,
    ) -> anyhow::Result<ArtifactReference> {
        let artifact = ArtifactReference::new(self.store.storage.new_id(), content.len())?;
        database
            .artifacts
            .put(transaction, &artifact.id, content.as_bytes())?;
        database
            .artifact_index
            .record(transaction, database.snapshots, snapshot, &artifact)?;
        snapshot.artifacts.push(artifact.clone());
        Ok(artifact)
    }

    pub fn read_artifact(&self, id: &str, range: ArtifactRange) -> anyhow::Result<ArtifactPage> {
        self.store.read(&self.id, |database| {
            let transaction = database.env.read_txn()?;
            let artifact = match database.owner(&transaction, &self.id)? {
                Some(owner) if owner == self.owner => {
                    database.artifact_index.get(&transaction, &self.id, id)
                }
                _ => Err(anyhow::anyhow!("Session {} ownership was lost", self.id)),
            }?;
            let bytes = database
                .artifacts
                .get(&transaction, id)?
                .ok_or_else(|| anyhow::anyhow!("Artifact {id} is missing"))?;
            range.page(artifact, std::str::from_utf8(bytes)?)
        })
    }
}

fn artifact_preview(artifact: &ArtifactReference, content: &str) -> String {
    format!(
        "{}\n[Full output: artifact {} ({} bytes). Use read_artifact for missing sections, up to {ARTIFACT_PAGE_BYTES} bytes per call. Batch independent ranges when the full output is needed.]",
        preview(content, INLINE_BYTES - 512),
        artifact.id,
        artifact.bytes,
    )
}
