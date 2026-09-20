use super::{Session, Snapshot};
use crate::context::{Checkpoint, RequestMode};
use clients::llm::{Message, Role};
use common_models::runtime_ids::TurnId;

pub struct Conversation {
    cache_key: String,
    history: Vec<Message>,
    deferred_input: Vec<Message>,
    checkpoint: Checkpoint,
    compact_turn: Option<TurnId>,
}

impl Conversation {
    pub fn new(history: Vec<Message>, session: Option<&Session>) -> Self {
        Self {
            cache_key: session
                .map(|session| session.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            history,
            deferred_input: Vec::new(),
            checkpoint: Default::default(),
            compact_turn: None,
        }
    }

    pub fn restored(snapshot: &Snapshot, workspace: Message) -> Self {
        Self {
            cache_key: snapshot.id.clone(),
            history: std::iter::once(workspace)
                .chain(snapshot.history.iter().skip(1).cloned())
                .collect(),
            deferred_input: snapshot.deferred_input.clone(),
            checkpoint: snapshot.context.clone(),
            compact_turn: None,
        }
    }

    pub fn cache_key(&self) -> &str {
        &self.cache_key
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }

    pub fn commit_checkpoint(&mut self, checkpoint: Checkpoint) {
        self.checkpoint = checkpoint;
    }

    pub fn append(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.history.extend(messages);
        self.history.append(&mut self.deferred_input);
    }

    pub fn push(&mut self, message: Message) {
        self.history.push(message);
    }

    pub fn defer(&mut self, message: Message) {
        self.deferred_input.push(message);
    }

    pub fn has_deferred_input(&self) -> bool {
        !self.deferred_input.is_empty()
    }

    pub fn relocate(&mut self, workspace: Message) {
        if let Some(initial) = self.history.first_mut() {
            *initial = workspace;
        }
    }

    pub fn compact(&mut self, turn: TurnId) {
        self.compact_turn = Some(turn);
    }

    pub fn request_mode(&self, turn: TurnId, mode: RequestMode) -> RequestMode {
        match self.compact_turn {
            Some(compact) if compact == turn => RequestMode::Compact,
            _ => mode,
        }
    }

    pub fn constraints(&self) -> impl Iterator<Item = String> + '_ {
        self.history
            .iter()
            .skip(1)
            .filter(|message| matches!(message.role, Role::User))
            .map(Message::text)
            .filter(|text| !text.is_empty())
    }
}
