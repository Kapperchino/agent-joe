use crate::{
    changes::FileVersion, execution::ExecutionScope, inventory::Inventory,
    workspace::WorkspacePolicy,
};
use anyhow::Context;
use common_models::knowledge::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

pub use knowledge_indexer::native_target;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    workspace: String,
    files: BTreeMap<SourcePath, String>,
}

impl Fingerprint {
    pub fn capture(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        Capture::new(workspace, &|| Ok(())).map(|capture| capture.fingerprint)
    }

    pub fn is_current(&self, workspace: &WorkspacePolicy) -> anyhow::Result<bool> {
        Ok(Self::capture(workspace)? == *self)
    }
}

struct Capture {
    fingerprint: Fingerprint,
    files: BTreeMap<SourcePath, FileVersion>,
}

impl Capture {
    fn new(
        workspace: &WorkspacePolicy,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        check()?;
        let inventory = Inventory::scan(workspace)?;
        match inventory.skipped == 0
            && workspace.permits_workspace_access(crate::workspace::Access::Read)
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Knowledge capture requires whole-workspace access and no unreadable discoverable files"
            )),
        }?;
        let mut files = BTreeMap::new();
        let mut bytes = 0usize;
        for path in inventory.files {
            check()?;
            let source_path = SourcePath::try_from(
                path.to_str()
                    .context("Knowledge paths must be UTF-8")?
                    .to_owned(),
            )?;
            let file = workspace.file_version(&path)?;
            bytes = bytes.saturating_add(file.bytes().len());
            match &file {
                FileVersion::File { .. } if bytes <= MAX_SOURCE_BYTES => {
                    files.insert(source_path, file);
                }
                _ => Err(anyhow::anyhow!(
                    "Knowledge inputs changed during capture or exceed 64 MiB"
                ))?,
            }
        }
        let fingerprint = Fingerprint {
            workspace: workspace.workspace_identity()?,
            files: files
                .iter()
                .map(|(path, file)| (path.clone(), file.fingerprint()))
                .collect(),
        };
        Ok(Self { fingerprint, files })
    }

    fn sources(&self) -> anyhow::Result<Vec<SourceFile>> {
        self.files
            .iter()
            .filter_map(|(path, file)| {
                file.text()
                    .ok()
                    .filter(|text| !text.contains('\0'))
                    .map(|text| SourceFile::new(path.clone(), text.to_owned()))
            })
            .collect()
    }

    fn check_graph(&self, graph: &SemanticGraph, profile: &SemanticProfile) -> anyhow::Result<()> {
        let sources: BTreeMap<_, _> = graph
            .data()
            .sources
            .iter()
            .map(|source| (source.path(), source.text()))
            .collect();
        match &graph.data().profile == profile {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Knowledge graph has a different semantic profile"
            )),
        }?;
        let expected: BTreeMap<_, _> = self
            .files
            .iter()
            .filter_map(|(path, file)| {
                file.text()
                    .ok()
                    .filter(|text| !text.contains('\0'))
                    .map(|text| (path, text))
            })
            .collect();
        match sources == expected {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Knowledge graph differs from the captured sources"
            )),
        }
    }
}

pub struct PreparedKnowledge {
    pub graph: SemanticGraph,
    pub fingerprint: Fingerprint,
}

pub async fn prepare(profile: SemanticProfile) -> anyhow::Result<PreparedKnowledge> {
    let owner = ExecutionScope::current();
    let workspace = owner.workspace()?;
    let cancellation = owner.cancel.child_token();
    let _cancel = cancellation.clone().drop_guard();
    let deadline = Instant::now() + Duration::from_secs(1800);
    owner
        .tasks
        .spawn_blocking(move || {
            let check = || match (cancellation.is_cancelled(), Instant::now() >= deadline) {
                (true, _) => Err(anyhow::anyhow!("Knowledge preparation cancelled")),
                (_, true) => Err(anyhow::anyhow!(
                    "Knowledge preparation exceeded its cooperative time limit"
                )),
                (false, false) => Ok(()),
            };
            let capture = Capture::new(&workspace, &check)?;
            let graph =
                knowledge_indexer::load_sources(capture.sources()?, profile.clone(), &check)?;
            capture.check_graph(&graph, &profile)?;
            let current = Capture::new(&workspace, &check)?;
            check()?;
            match capture.fingerprint == current.fingerprint {
                true => Ok(PreparedKnowledge {
                    graph,
                    fingerprint: capture.fingerprint,
                }),
                false => Err(anyhow::anyhow!(
                    "Workspace changed during preparation; no knowledge generation was published"
                )),
            }
        })
        .await?
}

#[cfg(all(test, unix))]
mod tests;
