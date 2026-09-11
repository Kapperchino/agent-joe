use crate::{
    git::{DiffTarget, GitRepository, GitStatus},
    workspace::{Access, WorkspacePolicy},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const SNAPSHOT_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileVersion {
    Missing,
    File { content: Vec<u8>, mode: u32 },
}

impl FileVersion {
    pub fn text(&self) -> anyhow::Result<&str> {
        match self {
            Self::Missing => Err(anyhow::anyhow!("File does not exist")),
            Self::File { content, .. } => Ok(std::str::from_utf8(content)?),
        }
    }

    pub fn with_text(&self, text: String) -> Self {
        Self::File {
            content: text.into_bytes(),
            mode: self.mode(),
        }
    }

    pub fn mode(&self) -> u32 {
        match self {
            Self::Missing => 0o644,
            Self::File { mode, .. } => *mode,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Missing => &[],
            Self::File { content, .. } => content,
        }
    }

    pub fn fingerprint(&self) -> String {
        let digest = blake3::hash(self.bytes());
        match self {
            Self::Missing => "missing".into(),
            Self::File { mode, .. } => format!("{mode:o}:{digest}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: PathBuf,
    pub before: FileVersion,
    pub after: FileVersion,
}

impl FileEdit {
    pub fn new(
        workspace: &WorkspacePolicy,
        path: &Path,
        before: FileVersion,
        after: FileVersion,
    ) -> anyhow::Result<Self> {
        let path = workspace.relative_path(path, Access::Write)?;
        Ok(Self {
            path,
            before,
            after,
        })
    }

    fn preflight(
        self,
        workspace: &WorkspacePolicy,
        observed: Option<&FileVersion>,
    ) -> anyhow::Result<Self> {
        match observed {
            Some(observed) if observed != &self.before => Err(anyhow::anyhow!(
                "Stale file {}; read it again before editing",
                self.path.display()
            )),
            _ if workspace.file_version(&self.path)? != self.before => Err(anyhow::anyhow!(
                "File changed before patch preflight: {}",
                self.path.display()
            )),
            _ => Ok(self),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditState {
    Prepared,
    Applying,
    Applied,
    Failed { message: String },
    Undone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRecord {
    pub id: String,
    pub edits: Vec<FileEdit>,
    pub applied: Vec<PathBuf>,
    pub in_flight: Option<PathBuf>,
    pub state: EditState,
    pub undo_of: Option<String>,
}

enum EditEvent {
    BeginFile,
    ConfirmFile,
    Complete,
    Fail { message: String },
    Undo,
}

impl EditRecord {
    fn prepared(
        workspace: &WorkspacePolicy,
        tracker: &TrackerState,
        edits: Vec<FileEdit>,
        undo_of: Option<String>,
    ) -> anyhow::Result<Self> {
        let edits = edits
            .into_iter()
            .map(|edit| FileEdit::new(workspace, &edit.path, edit.before, edit.after))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let bytes = edits
            .iter()
            .map(|edit| edit.before.bytes().len() + edit.after.bytes().len())
            .sum::<usize>();
        let paths = edits
            .iter()
            .map(|edit| edit.path.as_path())
            .collect::<BTreeSet<_>>();
        let overlapping = paths.len() != edits.len()
            || paths.iter().any(|path| {
                path.ancestors()
                    .skip(1)
                    .any(|parent| paths.contains(parent))
            });
        match edits {
            _ if bytes > SNAPSHOT_LIMIT || edits.is_empty() => Err(anyhow::anyhow!(
                "An edit must contain files and fit within 64 MiB"
            )),
            _ if overlapping => Err(anyhow::anyhow!(
                "A patch contains duplicate or overlapping paths"
            )),
            edits => {
                let edits = edits
                    .into_iter()
                    .map(|edit| {
                        let observed = match undo_of {
                            Some(_) => None,
                            None => tracker.observed_version(&edit.path),
                        };
                        edit.preflight(workspace, observed)
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                Ok(Self {
                    id: uuid::Uuid::new_v4().to_string(),
                    edits,
                    applied: Vec::new(),
                    in_flight: None,
                    state: EditState::Prepared,
                    undo_of,
                })
            }
        }
    }

    fn fully_confirmed(&self) -> bool {
        !self.edits.is_empty()
            && self.in_flight.is_none()
            && self
                .applied
                .iter()
                .eq(self.edits.iter().map(|edit| &edit.path))
    }

    fn transition(&mut self, event: EditEvent) -> anyhow::Result<()> {
        self.state = match (&self.state, event) {
            (EditState::Prepared | EditState::Applying, EditEvent::BeginFile)
                if self.in_flight.is_none() =>
            {
                let edit = self
                    .edits
                    .get(self.applied.len())
                    .ok_or_else(|| anyhow::anyhow!("No pending file in this edit"))?;
                self.in_flight = Some(edit.path.clone());
                EditState::Applying
            }
            (EditState::Applying, EditEvent::ConfirmFile) => {
                let path = self
                    .in_flight
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("No in-flight file to confirm"))?;
                self.applied.push(path);
                EditState::Applying
            }
            (EditState::Applying, EditEvent::Complete) if self.fully_confirmed() => {
                EditState::Applied
            }
            (EditState::Prepared | EditState::Applying, EditEvent::Fail { message }) => {
                EditState::Failed { message }
            }
            (EditState::Applied, EditEvent::Undo) if self.fully_confirmed() => EditState::Undone,
            _ => Err(anyhow::anyhow!(
                "Invalid edit transition from {:?}",
                self.state
            ))?,
        };
        Ok(())
    }

    fn undo_edits(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Vec<FileEdit>> {
        match self.state {
            EditState::Applied if self.fully_confirmed() => self
                .edits
                .iter()
                .rev()
                .map(|edit| {
                    FileEdit::new(
                        workspace,
                        &edit.path,
                        edit.after.clone(),
                        edit.before.clone(),
                    )
                })
                .collect(),
            _ => Err(anyhow::anyhow!(
                "Only fully applied, recorded Joe edits can be undone"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditSummary {
    pub id: String,
    pub paths: Vec<PathBuf>,
    pub applied: Vec<PathBuf>,
    pub in_flight: Option<PathBuf>,
    pub state: EditState,
    pub undo_of: Option<String>,
}

impl From<&EditRecord> for EditSummary {
    fn from(record: &EditRecord) -> Self {
        Self {
            id: record.id.clone(),
            paths: record.edits.iter().map(|edit| edit.path.clone()).collect(),
            applied: record.applied.clone(),
            in_flight: record.in_flight.clone(),
            state: record.state.clone(),
            undo_of: record.undo_of.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeSnapshot {
    pub baseline: Option<Baseline>,
    pub records: Vec<EditRecord>,
    #[serde(default)]
    pub worktrees: Vec<crate::git::worktrees::ManagedWorktree>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub workspace: String,
    pub files: BTreeMap<PathBuf, FileVersion>,
    pub git: Option<GitStatus>,
    pub staged: String,
    pub unstaged: String,
    pub index: Vec<crate::git::IndexEntry>,
}

pub trait ChangeStore: Send + Sync {
    fn save(&self, snapshot: &ChangeSnapshot) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct ChangeTracker {
    state: Mutex<TrackerState>,
}

#[derive(Default)]
struct TrackerState {
    snapshot: ChangeSnapshot,
    observed: BTreeMap<PathBuf, FileVersion>,
    store: Option<Arc<dyn ChangeStore>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    pub index_changes: Vec<IndexChange>,
    pub baseline_git: Option<GitStatus>,
    pub current_git: Option<GitStatus>,
    pub baseline_staged: String,
    pub staged: String,
    pub unstaged: String,
    pub changes: Vec<ReviewedFile>,
    pub edits: Vec<EditSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexChange {
    pub before: Option<crate::git::IndexEntry>,
    pub after: Option<crate::git::IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewedFile {
    pub path: PathBuf,
    pub ownership: ChangeOwnership,
    pub task_diff: String,
    pub joe_diff: String,
    pub current_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOwnership {
    Joe,
    External,
    JoeAndExternal,
    Uncertain,
}

impl ChangeTracker {
    pub fn restored(snapshot: ChangeSnapshot, store: Option<Arc<dyn ChangeStore>>) -> Self {
        Self {
            state: Mutex::new(TrackerState {
                snapshot,
                store,
                ..TrackerState::default()
            }),
        }
    }

    pub fn snapshot(&self) -> anyhow::Result<ChangeSnapshot> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?
            .snapshot
            .clone())
    }

    pub fn start(&self, workspace: &WorkspacePolicy) -> anyhow::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?;
        match &state.snapshot.baseline {
            Some(baseline) => match baseline.workspace == workspace.workspace_identity()? {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "The task baseline belongs to another workspace"
                )),
            },
            None => {
                let baseline = Baseline::capture(workspace)?;
                let mut snapshot = state.snapshot.clone();
                snapshot.baseline = Some(baseline);
                state.commit(snapshot)
            }
        }
    }

    pub fn observe(
        &self,
        workspace: &WorkspacePolicy,
        path: &Path,
        version: FileVersion,
    ) -> anyhow::Result<()> {
        let path = workspace.relative_path(path, Access::Read)?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?
            .observed
            .insert(path, version);
        Ok(())
    }

    pub fn apply(
        &self,
        workspace: &WorkspacePolicy,
        edits: Vec<FileEdit>,
    ) -> anyhow::Result<EditRecord> {
        self.apply_record(workspace, edits, None)
    }

    fn apply_record(
        &self,
        workspace: &WorkspacePolicy,
        edits: Vec<FileEdit>,
        undo_of: Option<String>,
    ) -> anyhow::Result<EditRecord> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?;
        let mut record = EditRecord::prepared(workspace, &state, edits, undo_of)?;
        let mut snapshot = state.snapshot.clone();
        snapshot.records.push(record.clone());
        state.commit(snapshot)?;
        let staged = record
            .edits
            .iter()
            .map(|edit| workspace.stage_edit(edit))
            .collect::<anyhow::Result<Vec<_>>>();
        let result = staged.and_then(|staged| {
            staged
                .into_iter()
                .enumerate()
                .try_for_each(|(index, staged)| {
                    record.transition(EditEvent::BeginFile)?;
                    state.record(record.clone())?;
                    let edit = &record.edits[index];
                    staged.apply(workspace, edit)?;
                    state.observed.insert(edit.path.clone(), edit.after.clone());
                    record.transition(EditEvent::ConfirmFile)?;
                    state.record(record.clone())
                })
        });
        record.transition(match &result {
            Ok(()) => EditEvent::Complete,
            Err(error) => EditEvent::Fail {
                message: format!("{error:#}"),
            },
        })?;
        state.record(record.clone())?;
        match result {
            Ok(()) => Ok(record),
            Err(error) => Err(anyhow::anyhow!(
                "Patch {} failed; applied paths: {:?}; {error:#}",
                record.id,
                record.applied
            )),
        }
    }

    pub fn undo(&self, workspace: &WorkspacePolicy, id: &str) -> anyhow::Result<EditRecord> {
        let snapshot = self.snapshot()?;
        let record = snapshot
            .records
            .iter()
            .find(|record| record.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Joe edit ID"))?;
        let edits = record.undo_edits(workspace)?;
        let result = self.apply_record(workspace, edits, Some(id.to_owned()))?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?;
        let mut original = record.clone();
        original.transition(EditEvent::Undo)?;
        state.record(original)?;
        Ok(result)
    }

    pub fn update_worktrees(
        &self,
        worktrees: Vec<crate::git::worktrees::ManagedWorktree>,
    ) -> anyhow::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("Change journal lock poisoned"))?;
        let mut snapshot = state.snapshot.clone();
        snapshot.worktrees = worktrees;
        state.commit(snapshot)
    }

    pub fn review(&self, workspace: &WorkspacePolicy) -> anyhow::Result<Review> {
        let snapshot = self.snapshot()?;
        let baseline = snapshot
            .baseline
            .ok_or_else(|| anyhow::anyhow!("No task baseline has been captured"))?;
        let current = Baseline::capture(workspace)?;
        let paths = baseline
            .files
            .keys()
            .chain(current.files.keys())
            .chain(
                snapshot
                    .records
                    .iter()
                    .flat_map(|record| record.edits.iter().map(|edit| &edit.path)),
            )
            .cloned()
            .collect::<BTreeSet<_>>();
        let changes = paths
            .into_iter()
            .map(|path| {
                let before = baseline
                    .files
                    .get(&path)
                    .cloned()
                    .unwrap_or(FileVersion::Missing);
                let after = workspace.file_version(&path)?;
                let intended = snapshot
                    .records
                    .iter()
                    .flat_map(|record| {
                        record.edits.iter().filter(|edit| {
                            record.applied.contains(&edit.path)
                                || record.in_flight.as_ref() == Some(&edit.path)
                        })
                    })
                    .filter(|edit| edit.path == path)
                    .collect::<Vec<_>>();
                let joe_before = intended.first().map(|edit| &edit.before).unwrap_or(&before);
                let joe_after = intended.last().map(|edit| &edit.after).unwrap_or(&before);
                let external_between_edits = intended
                    .windows(2)
                    .any(|pair| pair[0].after != pair[1].before)
                    || joe_before != &before;
                let uncertain = snapshot
                    .records
                    .iter()
                    .any(|record| record.in_flight.as_ref() == Some(&path));
                let ownership = match intended.as_slice() {
                    _ if uncertain => ChangeOwnership::Uncertain,
                    [] => ChangeOwnership::External,
                    _ if &after == joe_after && !external_between_edits => ChangeOwnership::Joe,
                    _ => ChangeOwnership::JoeAndExternal,
                };
                let changed = before != after || !intended.is_empty();
                Ok(changed.then(|| ReviewedFile {
                    task_diff: content_diff(&path, &before, &after),
                    joe_diff: intended
                        .iter()
                        .map(|edit| content_diff(&path, &edit.before, &edit.after))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    current_fingerprint: after.fingerprint(),
                    path,
                    ownership,
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let before_index = baseline
            .index
            .iter()
            .map(|entry| (entry.key(), entry))
            .collect::<BTreeMap<_, _>>();
        let after_index = current
            .index
            .iter()
            .map(|entry| (entry.key(), entry))
            .collect::<BTreeMap<_, _>>();
        let keys = before_index
            .keys()
            .chain(after_index.keys())
            .collect::<BTreeSet<_>>();
        let index_changes = keys
            .into_iter()
            .filter(|key| before_index.get(*key) != after_index.get(*key))
            .map(|key| IndexChange {
                before: before_index.get(key).map(|entry| (*entry).clone()),
                after: after_index.get(key).map(|entry| (*entry).clone()),
            })
            .collect();
        let review = Review {
            index_changes,
            baseline_git: baseline.git,
            current_git: current.git,
            baseline_staged: baseline.staged,
            staged: current.staged,
            unstaged: current.unstaged,
            changes,
            edits: snapshot.records.iter().map(EditSummary::from).collect(),
        };
        match serde_json::to_vec(&review)?.len() <= SNAPSHOT_LIMIT {
            true => Ok(review),
            false => Err(anyhow::anyhow!("Aggregate review exceeds 64 MiB")),
        }
    }
}

impl TrackerState {
    fn observed_version(&self, path: &Path) -> Option<&FileVersion> {
        self.observed
            .get(path)
            .or_else(|| {
                self.snapshot.records.iter().rev().find_map(|record| {
                    record
                        .edits
                        .iter()
                        .find(|edit| edit.path == path && record.applied.contains(&edit.path))
                        .map(|edit| &edit.after)
                })
            })
            .or_else(|| {
                self.snapshot
                    .baseline
                    .as_ref()
                    .map(|baseline| baseline.files.get(path).unwrap_or(&FileVersion::Missing))
            })
    }

    fn commit(&mut self, snapshot: ChangeSnapshot) -> anyhow::Result<()> {
        match serde_json::to_vec(&snapshot)?.len() <= SNAPSHOT_LIMIT {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Change journal exceeds 64 MiB")),
        }?;
        if let Some(store) = &self.store {
            store.save(&snapshot)?;
        }
        self.snapshot = snapshot;
        Ok(())
    }

    fn record(&mut self, record: EditRecord) -> anyhow::Result<()> {
        let mut snapshot = self.snapshot.clone();
        let index = snapshot
            .records
            .iter()
            .position(|saved| saved.id == record.id)
            .ok_or_else(|| anyhow::anyhow!("Missing prepared edit"))?;
        snapshot.records[index] = record;
        self.commit(snapshot)
    }
}

impl Baseline {
    pub fn capture(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        let git = GitRepository::open(workspace)?;
        let paths = match &git {
            Some(git) => git.paths(workspace)?,
            None => crate::inventory::Inventory::scan(workspace)?.files,
        };
        let mut total = 0usize;
        let files = paths
            .into_iter()
            .map(|path| {
                let version = workspace.file_version(&path)?;
                total += version.bytes().len();
                match total <= SNAPSHOT_LIMIT {
                    true => Ok((path, version)),
                    false => Err(anyhow::anyhow!("Task baseline exceeds 64 MiB")),
                }
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let index = git
            .as_ref()
            .map(|git| git.index(workspace))
            .transpose()?
            .unwrap_or_default();
        let status = git.as_ref().map(|git| git.status(workspace)).transpose()?;
        let staged = git
            .as_ref()
            .map(|git| git.diff(workspace, DiffTarget::Staged, None))
            .transpose()?
            .unwrap_or_default();
        let unstaged = git
            .as_ref()
            .map(|git| git.diff(workspace, DiffTarget::Unstaged, None))
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            workspace: workspace.workspace_identity()?,
            index,
            files,
            git: status,
            staged,
            unstaged,
        })
    }
}

pub fn content_diff(path: &Path, before: &FileVersion, after: &FileVersion) -> String {
    let old = match before {
        FileVersion::Missing => "/dev/null".into(),
        _ => format!("a/{}", path.display()),
    };
    let new = match after {
        FileVersion::Missing => "/dev/null".into(),
        _ => format!("b/{}", path.display()),
    };
    match (
        std::str::from_utf8(before.bytes()),
        std::str::from_utf8(after.bytes()),
    ) {
        (Ok(before_text), Ok(after_text))
            if !before.bytes().contains(&0) && !after.bytes().contains(&0) =>
        {
            let mode = match (before, after) {
                (FileVersion::Missing, FileVersion::File { mode, .. }) => {
                    format!("new file mode {mode:o}\n")
                }
                (FileVersion::File { mode, .. }, FileVersion::Missing) => {
                    format!("deleted file mode {mode:o}\n")
                }
                _ if before.mode() != after.mode() => format!(
                    "old mode {:o}\nnew mode {:o}\n",
                    before.mode(),
                    after.mode()
                ),
                _ => String::new(),
            };
            format!(
                "{mode}{}",
                similar::TextDiff::from_lines(before_text, after_text)
                    .unified_diff()
                    .header(&old, &new)
            )
        }
        _ => format!(
            "Binary file {}: {} -> {}\n",
            path.display(),
            before.fingerprint(),
            after.fingerprint()
        ),
    }
}

impl Review {
    pub fn render(&self) -> String {
        let changes = self
            .changes
            .iter()
            .map(|change| {
                format!(
                    "{} ({:?})\n```diff\n{}\n```",
                    change.path.display(),
                    change.ownership,
                    change.task_diff
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let baseline = serde_json::to_string_pretty(&self.baseline_git).unwrap_or_default();
        let current = serde_json::to_string_pretty(&self.current_git).unwrap_or_default();
        let edits = self
            .edits
            .iter()
            .map(|edit| format!("{}: {:?} {:?}", edit.id, edit.state, edit.paths))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "Baseline Git status\n```json\n{baseline}\n```\n\nCurrent Git status\n```json\n{current}\n```\n\nRecorded Joe edits\n{edits}\n\nTask changes\n\n{changes}\n\nCurrent staged changes\n```diff\n{}\n```\n\nCurrent unstaged and untracked changes\n```diff\n{}\n```",
            self.staged, self.unstaged
        )
    }
}

#[cfg(test)]
#[path = "../tests/unit/changes/tests.rs"]
mod tests;
