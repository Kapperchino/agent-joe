use clients::llm::{ContentBlock, Message, Role, SessionProvider};
use common_models::tui_models::{Lifecycle, SessionSummary, TokenCount};
use heed::{
    Database, Env, EnvOpenOptions,
    types::{Bytes, Str},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tools::{
    tool_defs::{ToolEffect, ToolResult},
    tool_error::{ToolEffects, ToolFailure},
};
use utils::workspace::{PrivateStorage, WorkspacePolicy};

mod ownership;
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

pub struct SessionStore {
    env: Env,
    snapshots: Database<Str, Bytes>,
    events: Database<Str, Bytes>,
    owners: Database<Str, Bytes>,
    storage: PrivateStorage,
}

pub(crate) struct Session {
    store: Arc<SessionStore>,
    pub id: String,
    owner: Owner,
}

pub(crate) struct ResumableSession {
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
        let mut transaction = store.env.write_txn()?;
        let snapshot = match identity == store.storage.workspace_identity() {
            true => store.snapshot(&transaction, id),
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
            _ => store.claim(&mut transaction, id),
        }?;
        transaction.commit()?;
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
        Ok(self.session)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    version: SchemaVersion,
    pub sequence: u64,
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
    pub updated_at: Option<std::time::SystemTime>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct QueuedInput {
    pub turn: String,
    pub prompt: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PendingBatch {
    pub assistant: Message,
    pub operations: Vec<Operation>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Operation {
    pub id: String,
    pub call: crate::tool_call::ToolCall,
    pub state: OperationState,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) enum OperationState {
    Queued,
    Intended { effect: ToolEffect },
    Completed(ToolResult),
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Event {
    Created,
    Queued(QueuedInput),
    Began(QueuedInput),
    History(Vec<Message>),
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
    pub fn open(workspace: &WorkspacePolicy, namespace: &str) -> anyhow::Result<Arc<Self>> {
        static OPEN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = OPEN
            .lock()
            .map_err(|_| anyhow::anyhow!("Session storage initialization lock poisoned"))?;
        let storage = workspace.session_storage(namespace)?;
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(1024 * 1024 * 1024)
                .max_dbs(3)
                .open(storage.path())?
        };
        let mut transaction = env.write_txn()?;
        let snapshots = env.create_database(&mut transaction, Some("session_snapshots"))?;
        let events = env.create_database(&mut transaction, Some("session_events"))?;
        let owners = env.create_database(&mut transaction, Some("session_owners"))?;
        transaction.commit()?;
        Ok(Arc::new(Self {
            env,
            snapshots,
            events,
            owners,
            storage,
        }))
    }

    pub(crate) fn create(
        self: &Arc<Self>,
        provider: SessionProvider,
        parent: Option<String>,
        history: Vec<Message>,
    ) -> anyhow::Result<Arc<Session>> {
        let id = self.storage.new_id();
        let snapshot = Snapshot {
            version: SchemaVersion,
            sequence: 1,
            id: id.clone(),
            workspace: self.storage.workspace_identity().to_owned(),
            provider,
            parent,
            history,
            pending: None,
            queued: Vec::new(),
            status: Lifecycle::Ready,
            usage: TokenCount::default(),
            updated_at: Some(std::time::SystemTime::now()),
        };
        let mut transaction = self.env.write_txn()?;
        let owner = self.claim(&mut transaction, &id)?;
        self.write(&mut transaction, &snapshot, Event::Created)?;
        transaction.commit()?;
        Ok(Arc::new(Session {
            store: self.clone(),
            id,
            owner,
        }))
    }

    fn owner(&self, transaction: &heed::RoTxn<'_>, id: &str) -> anyhow::Result<Option<Owner>> {
        self.owners.get(transaction, id)?.map(decode).transpose()
    }

    fn claim(&self, transaction: &mut heed::RwTxn<'_>, id: &str) -> anyhow::Result<Owner> {
        let owner = Owner::new(self.owner(transaction, id)?, self.storage.new_id())?;
        self.owners
            .put(transaction, id, &serde_json::to_vec(&owner)?)?;
        Ok(owner)
    }

    fn snapshot(&self, transaction: &heed::RoTxn<'_>, id: &str) -> anyhow::Result<Snapshot> {
        let bytes = self
            .snapshots
            .get(transaction, id)?
            .ok_or_else(|| anyhow::anyhow!("Session {id} does not exist"))?;
        decode(bytes)
    }

    pub(crate) fn list(&self) -> anyhow::Result<Vec<Snapshot>> {
        let transaction = self.env.read_txn()?;
        self.snapshots
            .iter(&transaction)?
            .map(|entry| {
                let (_, bytes) = entry?;
                decode(bytes)
            })
            .collect()
    }

    pub(crate) fn resume_choices(
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

    fn write(
        &self,
        transaction: &mut heed::RwTxn<'_>,
        snapshot: &Snapshot,
        event: Event,
    ) -> anyhow::Result<()> {
        let key = format!("{}:{:020}", snapshot.id, snapshot.sequence);
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
    pub fn key(&self, id: impl std::fmt::Display) -> String {
        format!("{}:{id}", self.owner.token)
    }

    pub fn snapshot(&self) -> anyhow::Result<Snapshot> {
        let transaction = self.store.env.read_txn()?;
        self.owned_snapshot(&transaction)
    }

    fn owned_snapshot(&self, transaction: &heed::RoTxn<'_>) -> anyhow::Result<Snapshot> {
        match self.store.owner(transaction, &self.id)? {
            Some(owner) if owner == self.owner => self.store.snapshot(transaction, &self.id),
            _ => Err(anyhow::anyhow!("Session {} ownership was lost", self.id)),
        }
    }

    pub fn record(&self, event: Event) -> anyhow::Result<()> {
        let mut transaction = self.store.env.write_txn()?;
        let mut snapshot = self.owned_snapshot(&transaction)?.transition(&event)?;
        snapshot.sequence += 1;
        snapshot.updated_at = Some(std::time::SystemTime::now());
        self.store.write(&mut transaction, &snapshot, event)?;
        transaction.commit()?;
        Ok(())
    }

    fn release(&self) -> anyhow::Result<()> {
        let mut transaction = self.store.env.write_txn()?;
        match self.store.owner(&transaction, &self.id)? {
            Some(owner) if owner == self.owner => {
                self.store.owners.delete(&mut transaction, &self.id)?;
                transaction.commit()?;
                Ok(())
            }
            _ => Ok(()),
        }
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
            Event::Created => Err(anyhow::anyhow!("Session already exists"))?,
            Event::Queued(input) => self.queued.push(input.clone()),
            Event::Began(input) => {
                self.queued.retain(|queued| queued.turn != input.turn);
                self.history.extend(input.prompt.clone().map(Message::new));
                self.status = Lifecycle::Running;
            }
            Event::History(messages) => {
                self.history.extend(messages.clone());
                self.pending = None;
            }
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
            }
            Event::Status { turn, state, .. } => {
                self.queued
                    .retain(|queued| !(queued.turn == *turn && state.terminal()));
                self.status = *state;
            }
            Event::Usage(usage) => self.usage = usage.clone(),
            Event::Recovered => {
                self.history.extend(
                    self.pending
                        .take()
                        .into_iter()
                        .flat_map(PendingBatch::messages),
                );
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
                    _ => Lifecycle::Cancelled,
                };
            }
        }
        Ok(self)
    }
}

impl PendingBatch {
    pub fn new(session: &Session, batch: &crate::turn::ToolBatch) -> Self {
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
    pub fn new(id: String, call: crate::tool_call::ToolCall) -> Self {
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
                tool_id: result.id,
                is_error: result.outcome.is_err().then_some(true),
                content: result.outcome.unwrap_or_else(|error| error.to_string()),
            },
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
