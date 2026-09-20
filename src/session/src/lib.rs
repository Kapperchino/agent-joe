use clients::llm::{ContentBlock, Message, Role, SessionProvider};
use common_models::tui_models::{Lifecycle, SessionSummary, TokenCount};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tools::{
    tool_defs::{ToolEffect, ToolResult},
    tool_error::{ToolEffects, ToolFailure},
};
use utils::workspace::WorkspacePolicy;

pub mod activation;
mod artifact_index;
pub mod artifacts;
pub mod changes;
pub mod control;
#[cfg(test)]
#[path = "../tests/unit/session_control_test.rs"]
mod control_tests;
mod generations;
mod ownership;
pub mod persistence;
mod prune;
pub mod runtime;
pub mod state;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod transition;
use generations::SessionDatabase;
pub use generations::SessionStore;
use ownership::Owner;

const VERSION: u32 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
struct SchemaVersion;

impl TryFrom<u32> for SchemaVersion {
    type Error = anyhow::Error;

    fn try_from(version: u32) -> anyhow::Result<Self> {
        match version {
            VERSION => Ok(Self),
            _ => Err(anyhow::anyhow!(
                "Unsupported session schema version {version}; expected {VERSION}"
            )),
        }
    }
}

impl From<SchemaVersion> for u32 {
    fn from(_: SchemaVersion) -> Self {
        VERSION
    }
}

pub struct Session {
    store: Arc<SessionStore>,
    pub id: String,
    owner: Owner,
}

pub struct ResumableSession {
    session: Arc<Session>,
}

impl ResumableSession {
    pub fn new(
        store: &Arc<SessionStore>,
        id: &str,
        workspace: &WorkspacePolicy,
        provider: &SessionProvider,
    ) -> anyhow::Result<Self> {
        let identity = workspace.workspace_identity()?;
        let owner = store.update(Some(id), |database| {
            let mut transaction = database.env.write_txn()?;
            let snapshot = match identity == store.storage.workspace_identity() {
                true => database.snapshot(&transaction, id),
                false => Err(anyhow::anyhow!(
                    "Session storage does not belong to the current workspace"
                )),
            }?;
            let owner = match snapshot {
                Snapshot {
                    workspace: saved, ..
                } if saved != identity => Err(anyhow::anyhow!(
                    "Session workspace identity does not match the current project"
                )),
                Snapshot {
                    provider: saved, ..
                } if &saved != provider => Err(anyhow::anyhow!(
                    "Session provider is incompatible with the current provider route"
                )),
                Snapshot {
                    parent: Some(_), ..
                } => Err(anyhow::anyhow!(
                    "Resume the parent session; worker sessions cannot be resumed interactively"
                )),
                _ => database.claim(&mut transaction, id),
            }?;
            transaction.commit()?;
            Ok(owner)
        })?;
        Ok(Self {
            session: Arc::new(Session {
                store: store.clone(),
                id: id.to_owned(),
                owner,
            }),
        })
    }

    pub fn resume(self) -> anyhow::Result<Arc<Session>> {
        self.session.record(Event::Recovered)?;
        self.session.archive_outputs()?;
        Ok(self.session)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    version: SchemaVersion,
    pub sequence: u64,
    #[serde(default)]
    pub changes: utils::changes::ChangeSnapshot,
    #[serde(default)]
    pub worktree: Option<utils::git::worktrees::session::SessionWorktree>,
    #[serde(default)]
    pub merge_approval: merge_workflow::MergeApproval,
    pub id: String,
    workspace: String,
    provider: SessionProvider,
    pub parent: Option<String>,
    pub history: Vec<Message>,
    pub pending: Option<PendingBatch>,
    pub queued: Vec<QueuedInput>,
    pub status: Lifecycle,
    pub usage: TokenCount,
    #[serde(default)]
    pub workers: std::collections::BTreeMap<String, worker_registry::report::WorkerView>,
    #[serde(default)]
    pub artifacts: Vec<artifacts::ArtifactReference>,
    #[serde(default)]
    pub forked_from: Option<String>,
    #[serde(default)]
    pub context: conversation::context::Checkpoint,
    #[serde(flatten)]
    pub questions: common_models::interaction::Questions,
    #[serde(default)]
    pub planning: common_models::interaction::Planning,
    #[serde(default)]
    pub deferred_input: Vec<Message>,
    #[serde(default)]
    pub updated_at: Option<std::time::SystemTime>,
    #[serde(default)]
    pub processes: std::collections::BTreeMap<String, utils::cargo::CargoResult>,
    #[serde(default)]
    process_reports: std::collections::BTreeSet<String>,
}

struct ForkableSnapshot(Snapshot);

impl TryFrom<Snapshot> for ForkableSnapshot {
    type Error = anyhow::Error;

    fn try_from(snapshot: Snapshot) -> anyhow::Result<Self> {
        match snapshot.pending.is_none()
            && snapshot.queued.is_empty()
            && (snapshot.status.terminal()
                || matches!(
                    snapshot.status,
                    Lifecycle::Ready | Lifecycle::WaitingForInput
                )) {
            true => Ok(Self(snapshot)),
            false => Err(anyhow::anyhow!(
                "Fork requires an idle session with no pending operations"
            )),
        }
    }
}

struct CompactionTransition {
    context: conversation::context::Checkpoint,
    usage: TokenCount,
}

impl CompactionTransition {
    fn new(
        snapshot: &Snapshot,
        context: &conversation::context::Checkpoint,
        usage: &TokenCount,
    ) -> anyhow::Result<Self> {
        let memory = context
            .memory
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Compaction requires saved memory"))?;
        match context.through > snapshot.context.through
            && snapshot.context.generation.checked_add(1) == Some(context.generation)
            && snapshot.pending.is_none()
        {
            true => Ok(Self {
                context: conversation::context::Checkpoint::new(
                    &snapshot.history,
                    context.through,
                    context.generation,
                    memory,
                )?,
                usage: usage.clone(),
            }),
            false => Err(anyhow::anyhow!(
                "Compaction conflicts with the current session state"
            )),
        }
    }
}

pub use common_models::interaction::Question as PendingQuestion;

#[derive(Clone, Serialize, Deserialize)]
pub struct QueuedInput {
    pub turn: String,
    pub prompt: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PendingBatch {
    pub assistant: Message,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub call: clients::response::ToolCall,
    pub state: OperationState,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum OperationState {
    Queued,
    Intended { effect: ToolEffect },
    Completed(ToolResult),
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Event {
    Worktree(Option<utils::git::worktrees::session::SessionWorktree>),
    WorktreePruned,
    MergeApproval(merge_workflow::MergeApproval),
    Planning(common_models::interaction::Planning),
    Worker(Box<worker_registry::report::WorkerView>),
    Created,
    Changes(utils::changes::ChangeSnapshot),
    Forked {
        source: String,
    },
    Compacted {
        context: conversation::context::Checkpoint,
        usage: TokenCount,
    },
    QuestionAsked(PendingQuestion),
    QuestionsWithdrawn(common_models::interaction::QuestionPurpose),
    QuestionAnswered {
        id: String,
        answer: common_models::interaction::Answer,
    },
    Queued(QueuedInput),
    Began(QueuedInput),
    History(Vec<Message>),
    OutputsArchived(Vec<Message>),
    Prepared(PendingBatch),
    Intent {
        operation: String,
        effect: ToolEffect,
    },
    Completed {
        operation: String,
        result: ToolResult,
    },
    Status {
        turn: String,
        state: Lifecycle,
        detail: Option<String>,
    },
    ProcessCompleted(Box<utils::cargo::CargoResult>),
    Usage(TokenCount),
    Recovered,
}

#[derive(Serialize, Deserialize)]
struct Record {
    version: SchemaVersion,
    sequence: u64,
    event: Event,
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> anyhow::Result<T> {
    #[derive(Deserialize)]
    struct Header {
        #[serde(rename = "version")]
        _version: SchemaVersion,
    }
    let _: Header = serde_json::from_slice(bytes)?;
    serde_json::from_slice(bytes).map_err(Into::into)
}

impl SessionStore {
    pub fn create(
        self: &Arc<Self>,
        provider: SessionProvider,
        parent: Option<String>,
        history: Vec<Message>,
    ) -> anyhow::Result<Arc<Session>> {
        let id = self.storage.new_id();
        let snapshot = Snapshot {
            version: SchemaVersion,
            sequence: 1,
            changes: Default::default(),
            worktree: None,
            merge_approval: Default::default(),
            id: id.clone(),
            workspace: self.storage.workspace_identity().to_owned(),
            provider,
            parent,
            history,
            pending: None,
            queued: Vec::new(),
            status: Lifecycle::Ready,
            usage: TokenCount::default(),
            workers: Default::default(),
            artifacts: Vec::new(),
            forked_from: None,
            context: conversation::context::Checkpoint::default(),
            questions: Default::default(),
            planning: Default::default(),
            deferred_input: Vec::new(),
            updated_at: Some(std::time::SystemTime::now()),
            processes: Default::default(),
            process_reports: Default::default(),
        };
        let owner = self.update(snapshot.parent.as_deref(), |database| {
            let mut snapshot = snapshot.clone();
            let mut transaction = database.env.write_txn()?;
            if let Some(parent) = &snapshot.parent {
                database.snapshot(&transaction, parent)?;
                snapshot.artifacts = database.artifact_index.list(&transaction, parent)?;
            }
            let owner = database.claim(&mut transaction, &id)?;
            database.write(&mut transaction, &snapshot, Event::Created)?;
            transaction.commit()?;
            Ok(owner)
        })?;
        Ok(Arc::new(Session {
            store: self.clone(),
            id,
            owner,
        }))
    }

    pub fn resume_choices(
        &self,
        provider: &SessionProvider,
        current: Option<&str>,
    ) -> anyhow::Result<Vec<SessionSummary>> {
        let mut choices = self
            .list()?
            .into_iter()
            .filter(|snapshot| {
                snapshot.parent.is_none()
                    && &snapshot.provider == provider
                    && current != Some(snapshot.id.as_str())
            })
            .filter_map(|snapshot| snapshot.summary())
            .collect::<Vec<_>>();
        choices.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(choices)
    }
}

impl SessionDatabase {
    pub(crate) fn owner(
        &self,
        transaction: &heed::RoTxn<'_>,
        id: &str,
    ) -> anyhow::Result<Option<Owner>> {
        self.owners.get(transaction, id)?.map(decode).transpose()
    }

    fn claim(&self, transaction: &mut heed::RwTxn<'_>, id: &str) -> anyhow::Result<Owner> {
        let owner = Owner::new(self.owner(transaction, id)?, self.storage.new_id())?;
        self.owners
            .put(transaction, id, &serde_json::to_vec(&owner)?)?;
        Ok(owner)
    }

    pub(crate) fn snapshot(
        &self,
        transaction: &heed::RoTxn<'_>,
        id: &str,
    ) -> anyhow::Result<Snapshot> {
        let bytes = self
            .snapshots
            .get(transaction, id)?
            .ok_or_else(|| anyhow::anyhow!("Session {id} does not exist"))?;
        decode(bytes)
    }

    pub fn list(&self) -> anyhow::Result<Vec<Snapshot>> {
        let transaction = self.env.read_txn()?;
        self.snapshots
            .iter(&transaction)?
            .map(|entry| {
                let (_, bytes) = entry?;
                decode(bytes)
            })
            .collect()
    }

    fn write(
        &self,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &Snapshot,
        event: Event,
    ) -> anyhow::Result<()> {
        let key = format!("{}:{:020}", snapshot.id, snapshot.sequence);
        if matches!(event, Event::Created | Event::Forked { .. }) {
            self.artifact_index
                .inherit(transaction, &snapshot.id, &snapshot.artifacts)?;
        }
        let record = Record {
            version: SchemaVersion,
            sequence: snapshot.sequence,
            event,
        };
        self.events
            .put(transaction, &key, &serde_json::to_vec(&record)?)?;
        self.snapshots
            .put(transaction, &snapshot.id, &serde_json::to_vec(snapshot)?)?;
        Ok(())
    }
}

impl Session {
    pub fn fork(&self) -> anyhow::Result<Arc<Session>> {
        self.store.update(Some(&self.id), |database| {
            let mut transaction = database.env.write_txn()?;
            let ForkableSnapshot(mut snapshot) =
                ForkableSnapshot::try_from(self.owned_snapshot(database, &transaction)?)?;
            snapshot.artifacts = database.artifact_index.list(&transaction, &snapshot.id)?;
            snapshot.id = self.store.storage.new_id();
            snapshot.sequence = 1;
            snapshot.changes = Default::default();
            snapshot.worktree = None;
            snapshot.merge_approval = Default::default();
            snapshot
                .questions
                .withdraw(common_models::interaction::QuestionPurpose::Merge);
            snapshot.parent = None;
            snapshot.forked_from = Some(self.id.clone());
            snapshot.status = Lifecycle::Ready;
            snapshot.updated_at = Some(std::time::SystemTime::now());
            let owner = database.claim(&mut transaction, &snapshot.id)?;
            database.write(
                &mut transaction,
                &snapshot,
                Event::Forked {
                    source: self.id.clone(),
                },
            )?;
            transaction.commit()?;
            Ok(Arc::new(Session {
                store: self.store.clone(),
                id: snapshot.id,
                owner,
            }))
        })
    }

    pub fn key(&self, id: impl std::fmt::Display) -> String {
        format!("{}:{id}", self.owner.token)
    }

    pub fn snapshot(&self) -> anyhow::Result<Snapshot> {
        self.store.read(&self.id, |database| {
            let transaction = database.env.read_txn()?;
            self.owned_snapshot(database, &transaction)
        })
    }

    fn owned_snapshot(
        &self,
        database: &SessionDatabase,
        transaction: &heed::RoTxn<'_>,
    ) -> anyhow::Result<Snapshot> {
        match database.owner(transaction, &self.id)? {
            Some(owner) if owner == self.owner => database.snapshot(transaction, &self.id),
            _ => Err(anyhow::anyhow!("Session {} ownership was lost", self.id)),
        }
    }

    pub fn record(&self, event: Event) -> anyhow::Result<()> {
        self.store.update(Some(&self.id), |database| {
            let transaction = database.env.write_txn()?;
            let snapshot = self.owned_snapshot(database, &transaction)?;
            self.commit_event(database, transaction, snapshot, event.clone())
        })
    }

    fn commit_event(
        &self,
        database: &SessionDatabase,
        mut transaction: heed::RwTxn<'_>,
        snapshot: Snapshot,
        event: Event,
    ) -> anyhow::Result<()> {
        let mut snapshot = snapshot.transition(&event)?;
        snapshot.sequence += 1;
        snapshot.updated_at = Some(std::time::SystemTime::now());
        database.write(&mut transaction, &snapshot, event)?;
        transaction.commit()?;
        Ok(())
    }

    fn release(&self) -> anyhow::Result<()> {
        self.store.update(Some(&self.id), |database| {
            let mut transaction = database.env.write_txn()?;
            match database.owner(&transaction, &self.id)? {
                Some(owner) if owner == self.owner => {
                    database.owners.delete(&mut transaction, &self.id)?;
                    transaction.commit()?;
                    Ok(())
                }
                _ => Ok(()),
            }
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            tracing::warn!(session = self.id, %error, "Failed to release session ownership");
        }
    }
}

impl Snapshot {
    fn summary(&self) -> Option<SessionSummary> {
        let title = self
            .history
            .iter()
            .skip(1)
            .filter(|message| matches!(message.role, Role::User))
            .map(Message::text)
            .find(|text| !text.trim().is_empty())?;
        let preview = self
            .history
            .iter()
            .skip(1)
            .rev()
            .map(Message::text)
            .find(|text| !text.trim().is_empty())
            .unwrap_or_default();
        Some(SessionSummary {
            id: self.id.clone(),
            title: title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(100)
                .collect(),
            preview: preview.chars().take(500).collect(),
            updated_at: self.updated_at,
            status: self.status,
        })
    }

    fn operation(&mut self, id: &str) -> anyhow::Result<&mut Operation> {
        self.pending
            .as_mut()
            .and_then(|batch| {
                batch
                    .operations
                    .iter_mut()
                    .find(|operation| operation.id == id)
            })
            .ok_or_else(|| anyhow::anyhow!("Unknown session operation {id}"))
    }

    fn transition(mut self, event: &Event) -> anyhow::Result<Self> {
        match event {
            Event::Planning(planning) => self.planning = planning.clone(),
            Event::Worker(worker) => {
                self.workers
                    .insert(worker.worker_id.clone(), worker.as_ref().clone());
            }
            Event::Changes(changes) => self.changes = changes.clone(),
            Event::Worktree(worktree) => {
                if self.worktree.is_none() && worktree.is_some() {
                    self.changes = Default::default();
                }
                self.worktree = worktree.clone();
            }
            Event::WorktreePruned => {
                self.worktree = None;
                self.changes = Default::default();
                self.merge_approval = Default::default();
                self.questions
                    .withdraw(common_models::interaction::QuestionPurpose::Merge);
                self.history.push(Message::new(
                    "The session worktree was pruned. Its unmerged commits and local files were discarded. Resuming creates a fresh worktree from current main; previous edits and merge approvals no longer apply.".into(),
                ));
            }
            Event::MergeApproval(approval) => self.merge_approval = approval.clone(),
            Event::Created | Event::Forked { .. } => {
                Err(anyhow::anyhow!("Session already exists"))?
            }
            Event::Compacted { context, usage } => {
                let transition = CompactionTransition::new(&self, context, usage)?;
                self.context = transition.context;
                self.usage = transition.usage;
            }
            Event::QuestionAsked(question) => self.questions.ask(question.clone())?,
            Event::QuestionsWithdrawn(purpose) => self.questions.withdraw(*purpose),
            Event::QuestionAnswered { id, answer } => {
                let answered = self.questions.answer(id, answer)?;
                self.planning = self.planning.with_answer(&answered)?;
                let message = Message::new(answered.to_string());
                match &self.pending {
                    Some(_) => self.deferred_input.push(message),
                    None => self.history.push(message),
                }
            }
            Event::Queued(input) => self.queued.push(input.clone()),
            Event::Began(input) => {
                self.queued.retain(|queued| queued.turn != input.turn);
                self.history.extend(input.prompt.clone().map(Message::new));
                self.status = Lifecycle::Running;
            }
            Event::History(messages) => {
                self.history.extend(messages.clone());
                self.history.append(&mut self.deferred_input);
                self.pending = None;
            }
            Event::OutputsArchived(messages) => self.history = messages.clone(),
            Event::Prepared(batch) => {
                self.pending = match self.pending {
                    None => Ok(Some(batch.clone())),
                    Some(_) => Err(anyhow::anyhow!("The previous tool batch is still pending")),
                }?;
            }
            Event::Intent { operation, effect } => {
                self.operation(operation)?.intend(*effect)?;
            }
            Event::Completed { operation, result } => {
                self.operation(operation)?.complete(result)?;
                let content = match &result.outcome {
                    Ok(content) => content,
                    Err(failure) => &failure.message,
                };
                if let Ok(cargo) = serde_json::from_str::<utils::cargo::CargoResult>(content)
                    && let Some(id) = &cargo.process_id
                {
                    self.processes.entry(id.clone()).or_insert(cargo);
                }
            }
            Event::Status { turn, state, .. } => {
                self.queued
                    .retain(|queued| !(queued.turn == *turn && state.terminal()));
                self.status = *state;
            }
            Event::ProcessCompleted(result) => {
                let id = result
                    .process_id
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Managed result requires a process ID"))?;
                self.process_reports.remove(id);
                self.processes.insert(id.clone(), result.as_ref().clone());
            }
            Event::Usage(usage) => self.usage = usage.clone(),
            Event::Recovered => {
                for worker in self.workers.values_mut() {
                    worker.recover();
                }
                for (id, process) in &mut self.processes {
                    if process.status == sandbox::process::ProcessStatus::Running {
                        process.status = sandbox::process::ProcessStatus::Failed;
                        process.error = Some("Process completion is unknown after restart; saved IDs cannot be polled, stopped or relaunched".into());
                    }
                    if self.process_reports.insert(id.clone()) {
                        self.history.push(Message::new(format!("Saved managed process evidence; IDs cannot be reused after restart: {}", serde_json::to_string(process)?)));
                    }
                }
                self.history.extend(
                    self.pending
                        .take()
                        .into_iter()
                        .flat_map(PendingBatch::messages),
                );
                self.history.append(&mut self.deferred_input);
                self.history.extend(
                    self.queued
                        .drain(..)
                        .filter_map(|input| input.prompt)
                        .map(Message::new),
                );
                self.status = match self.status {
                    Lifecycle::Ready
                    | Lifecycle::Completed
                    | Lifecycle::Cancelled
                    | Lifecycle::Failed => self.status,
                    Lifecycle::WaitingForInput
                        if self.questions.gate()
                            == common_models::interaction::QuestionGate::Required =>
                    {
                        Lifecycle::WaitingForInput
                    }
                    _ => Lifecycle::Cancelled,
                };
            }
        }
        Ok(self)
    }
}

impl PendingBatch {
    pub fn new(session: &Session, batch: &turn_engine::turn::ToolBatch) -> Self {
        Self {
            assistant: batch.assistant_message(),
            operations: batch
                .jobs()
                .into_iter()
                .map(|job| Operation::new(session.key(job.operation), job.call))
                .collect(),
        }
    }

    pub fn messages(self) -> [Message; 2] {
        [
            self.assistant,
            Message {
                role: Role::User,
                content: self
                    .operations
                    .into_iter()
                    .map(Operation::result_content)
                    .collect(),
            },
        ]
    }
}

impl Operation {
    pub fn new(id: String, call: clients::response::ToolCall) -> Self {
        Self {
            id,
            call,
            state: OperationState::Queued,
        }
    }

    fn intend(&mut self, effect: ToolEffect) -> anyhow::Result<()> {
        self.state = match self.state {
            OperationState::Queued => Ok(OperationState::Intended { effect }),
            _ => Err(anyhow::anyhow!(
                "Operation intent already recorded; execution cannot be repeated"
            )),
        }?;
        Ok(())
    }

    fn complete(&mut self, result: &ToolResult) -> anyhow::Result<()> {
        self.state = match &self.state {
            OperationState::Completed(_) => {
                Err(anyhow::anyhow!("Operation completion already recorded"))
            }
            _ if self.call.id != result.id
                || self.call.name != result.invocation.name
                || self.call.input != result.invocation.input =>
            {
                Err(anyhow::anyhow!(
                    "Operation completion does not match the saved call"
                ))
            }
            OperationState::Queued
                if !matches!(
                    &result.outcome,
                    Err(ToolFailure {
                        effects: ToolEffects::NotStarted,
                        ..
                    })
                ) =>
            {
                Err(anyhow::anyhow!(
                    "Operation completion requires a committed intent"
                ))
            }
            _ => Ok(OperationState::Completed(result.clone())),
        }?;
        Ok(())
    }

    fn result_content(self) -> ContentBlock {
        match self.state {
            OperationState::Queued => self.call.error_content("Not executed: the session stopped before this tool started."),
            OperationState::Intended { .. } => self.call.error_content("Uncertain operation after restart: completion was not recorded. Inspect the workspace before retrying; do not repeat this operation blindly."),
            OperationState::Completed(result) => ContentBlock::ToolResult {
                content: result.content(),
                tool_id: result.id,
                is_error: result.outcome.is_err().then_some(true),
            },
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/session/tests.rs"]
pub mod tests;

impl utils::changes::ChangeStore for Session {
    fn save(&self, snapshot: &utils::changes::ChangeSnapshot) -> anyhow::Result<()> {
        self.record(Event::Changes(snapshot.clone()))
    }
}

struct SessionChanges {
    session: std::sync::Weak<Session>,
}

impl utils::changes::ChangeStore for SessionChanges {
    fn save(&self, snapshot: &utils::changes::ChangeSnapshot) -> anyhow::Result<()> {
        let session = self
            .session
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("The owning task session has closed"))?;
        utils::changes::ChangeStore::save(session.as_ref(), snapshot)
    }
}

impl Session {
    pub fn change_tracker(
        self: &Arc<Self>,
        snapshot: utils::changes::ChangeSnapshot,
    ) -> Arc<utils::changes::ChangeTracker> {
        Arc::new(utils::changes::ChangeTracker::restored(
            snapshot,
            Some(Arc::new(SessionChanges {
                session: Arc::downgrade(self),
            })),
        ))
    }
}
