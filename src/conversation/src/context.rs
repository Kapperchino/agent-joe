use clients::{
    compaction::CompactedWindow,
    llm::{ClientRequest, ContentBlock, Message, Role},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tools::tool_defs::ToolDefinition;

pub use clients::response::RequestMode;

const REQUEST_TOKEN_RESERVE: usize = 1024;
const MESSAGE_TOKEN_RESERVE: usize = 32;

pub struct TextPrompt<'a> {
    request: &'a ClientRequest,
}

impl<'a> TextPrompt<'a> {
    pub fn new(request: &'a ClientRequest) -> anyhow::Result<Self> {
        let text_only = request.tools.is_empty()
            && request
                .messages
                .iter()
                .flat_map(|message| &message.content)
                .all(|block| matches!(block, ContentBlock::MessageBlock { .. }));
        match text_only {
            true => Ok(Self { request }),
            false => Err(anyhow::anyhow!(
                "A text prompt requires text messages without tools"
            )),
        }
    }

    pub fn estimated_tokens(&self) -> usize {
        let tokenizer = tiktoken_rs::o200k_base_singleton();
        let content = self
            .request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::MessageBlock { text, .. } => Some(text),
                _ => None,
            })
            .map(|text| tokenizer.count_ordinary(text) + MESSAGE_TOKEN_RESERVE)
            .sum::<usize>();
        REQUEST_TOKEN_RESERVE
            + tokenizer.count_ordinary(self.request.system.as_deref().unwrap_or_default())
            + self.request.messages.len() * MESSAGE_TOKEN_RESERVE
            + content
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ContextLimits {
    ceiling: usize,
    response: u32,
}

impl ContextLimits {
    pub fn new(ceiling: usize, response: u32) -> anyhow::Result<Self> {
        match (ceiling, response) {
            (4096.., 1024..) if (response as usize) < ceiling / 2 => Ok(Self { ceiling, response }),
            _ => Err(anyhow::anyhow!(
                "Context ceiling must be at least 4096; response reserve must be at least 1024 and less than half the ceiling"
            )),
        }
    }

    pub fn ceiling(self) -> usize {
        self.ceiling
    }
    pub fn response(self) -> u32 {
        self.response
    }
    pub fn input(self) -> usize {
        self.ceiling - self.response as usize
    }
    pub fn trigger(self) -> usize {
        (self.ceiling - self.ceiling.div_ceil(10)).min(self.input())
    }
    pub fn summary_bytes(self) -> usize {
        (self.input() / 8).min(8192)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ContextBudget {
    Model { response: u32 },
    Fixed(ContextLimits),
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::Model { response: 16_000 }
    }
}

impl ContextBudget {
    pub fn new(ceiling: Option<usize>, response: u32) -> anyhow::Result<Self> {
        match ceiling {
            Some(ceiling) => ContextLimits::new(ceiling, response).map(Self::Fixed),
            None if response >= 1024 => Ok(Self::Model { response }),
            None => Err(anyhow::anyhow!("Response reserve must be at least 1024")),
        }
    }

    pub fn resolve(self, context_window: usize) -> anyhow::Result<ContextLimits> {
        match self {
            Self::Model { response } => ContextLimits::new(context_window, response),
            Self::Fixed(limits) => Ok(limits),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum NativeCompaction {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

impl std::str::FromStr for NativeCompaction {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> anyhow::Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "on" => Ok(Self::Enabled),
            "off" => Ok(Self::Disabled),
            _ => Err(anyhow::anyhow!(
                "Native compaction must be auto, on, or off"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Memory {
    Summary(String),
    Native(CompactedWindow),
}

impl Memory {
    pub fn summary(text: String, limits: ContextLimits) -> anyhow::Result<Self> {
        match text.trim() {
            "" => Err(anyhow::anyhow!(
                "Summary was empty or exceeded its requested size"
            )),
            _ if text.len() <= limits.summary_bytes() => Ok(Self::Summary(text)),
            _ => Err(anyhow::anyhow!(
                "Summary was empty or exceeded its requested size"
            )),
        }
    }

    fn message(&self) -> Message {
        match self {
            Self::Summary(summary) => Message::new(format!(
                "Historical conversation summary. Current instructions and later user messages take precedence.\n{summary}"
            )),
            Self::Native(window) => Message {
                role: Role::Assistant,
                content: vec![ContentBlock::OpenAICompaction(window.clone())],
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub through: usize,
    pub generation: u64,
    pub memory: Option<Memory>,
    #[serde(default)]
    pub runtime_from: usize,
}

impl Default for Checkpoint {
    fn default() -> Self {
        Self {
            through: 1,
            generation: 0,
            memory: None,
            runtime_from: 0,
        }
    }
}

impl Checkpoint {
    pub fn new(
        history: &[Message],
        through: usize,
        generation: u64,
        memory: Memory,
    ) -> anyhow::Result<Self> {
        let exchanges = CompleteHistory::new(history)?;
        match through {
            2.. if exchanges.ends.contains(&through) => Ok(Self {
                through,
                generation,
                memory: Some(memory),
                runtime_from: history.len(),
            }),
            _ => Err(anyhow::anyhow!(
                "Compaction must end at a complete exchange"
            )),
        }
    }
}

pub struct CompleteHistory {
    pub ends: Vec<usize>,
}

impl CompleteHistory {
    pub fn new(history: &[Message]) -> anyhow::Result<Self> {
        let scanned = history.iter().enumerate().skip(1).try_fold(
            HistoryScan {
                exchange: ExchangeState::Complete,
                ends: Vec::new(),
            },
            |scan, (index, message)| scan.next(index, message),
        )?;
        match scanned.exchange {
            ExchangeState::Complete => Ok(Self { ends: scanned.ends }),
            ExchangeState::Pending { .. } => Err(anyhow::anyhow!(
                "Unanswered tool calls cannot enter request context"
            )),
        }
    }
}

struct HistoryScan {
    exchange: ExchangeState,
    ends: Vec<usize>,
}

impl HistoryScan {
    fn next(mut self, index: usize, message: &Message) -> anyhow::Result<Self> {
        self.exchange = message
            .content
            .iter()
            .try_fold(self.exchange, |exchange, content| {
                exchange.next(&message.role, content)
            })?;
        let completes_message = match message.role {
            Role::Assistant => true,
            _ => message
                .content
                .iter()
                .any(|content| matches!(content, ContentBlock::ToolResult { .. })),
        };
        if let (ExchangeState::Complete, true) = (&self.exchange, completes_message) {
            self.ends.push(index + 1);
        }
        Ok(self)
    }
}

enum ExchangeState {
    Complete,
    Pending {
        seen: BTreeSet<String>,
        waiting: BTreeSet<String>,
    },
}

impl ExchangeState {
    fn next(self, role: &Role, content: &ContentBlock) -> anyhow::Result<Self> {
        match (role, content) {
            (Role::Assistant, ContentBlock::ToolBlock { tool_id, .. }) => {
                let id = tool_id
                    .call_id
                    .as_ref()
                    .unwrap_or(&tool_id.id)
                    .as_ref()
                    .to_owned();
                match self {
                    Self::Complete => Ok(Self::Pending {
                        seen: BTreeSet::from([id.clone()]),
                        waiting: BTreeSet::from([id]),
                    }),
                    Self::Pending { seen, .. } if seen.contains(&id) => {
                        Err(anyhow::anyhow!("Duplicate tool call in session history"))
                    }
                    Self::Pending {
                        mut seen,
                        mut waiting,
                    } => {
                        seen.insert(id.clone());
                        waiting.insert(id);
                        Ok(Self::Pending { seen, waiting })
                    }
                }
            }
            (Role::User, ContentBlock::ToolResult { tool_id, .. }) => {
                let id = tool_id.call_id.as_ref().unwrap_or(&tool_id.id).as_ref();
                match self {
                    Self::Pending { seen, mut waiting } => match waiting.take(id) {
                        Some(_) if waiting.is_empty() => Ok(Self::Complete),
                        Some(_) => Ok(Self::Pending { seen, waiting }),
                        None => Err(anyhow::anyhow!("Orphan tool result in session history")),
                    },
                    _ => Err(anyhow::anyhow!("Orphan tool result in session history")),
                }
            }
            (_, ContentBlock::ToolBlock { .. } | ContentBlock::ToolResult { .. }) => Err(
                anyhow::anyhow!("Misplaced tool call or result in session history"),
            ),
            (Role::User, _) if matches!(self, Self::Pending { .. }) => Err(anyhow::anyhow!(
                "User input interrupts an incomplete tool exchange"
            )),
            _ => Ok(self),
        }
    }
}

#[derive(Clone)]
pub struct ContextInput {
    pub runtime: Option<clients::runtime_update::RuntimeSnapshot>,
    pub prompt_cache_key: Option<String>,
    pub purpose: clients::llm::RequestPurpose,
    pub history: Vec<Message>,
    pub checkpoint: Checkpoint,
    pub instructions: String,
    pub tools: Vec<ToolDefinition>,
    pub limits: ContextLimits,
    pub native: NativeCompaction,
    pub mode: RequestMode,
}

pub enum BudgetPlan {
    Ready(ClientRequest),
    Compact(CompactionPlan),
}

pub struct CompactionPlan {
    pub through: usize,
    pub request: ClientRequest,
}

impl CompactionPlan {
    fn new(input: &ContextInput, exchanges: &CompleteHistory) -> anyhow::Result<Self> {
        let through = exchanges.ends.iter().rev().nth(2).copied()
                    .filter(|through| *through > input.checkpoint.through)
                    .ok_or_else(|| anyhow::anyhow!("No older complete exchanges can be compacted while retaining the two most recent exchanges. Shorten the latest input or use a larger --context-tokens limit."))?;
        let messages = input.prefix(&input.checkpoint, through)?;
        let request = ClientRequest::new(messages)
            .with_system(input.instructions.clone())
            .with_prompt_cache_key(input.prompt_cache_key.clone())
            .with_purpose(clients::llm::RequestPurpose::Compaction)
            .with_output_limit(input.limits.response());
        match estimated_tokens(&request)? <= input.limits.input() {
            true => Ok(Self { through, request }),
            false => Err(anyhow::anyhow!(
                "The older context exceeds the compaction input budget. Resume with a larger --context-tokens limit or start a new session; saved history is intact."
            )),
        }
    }
}

impl ContextInput {
    pub fn plan(&self) -> anyhow::Result<BudgetPlan> {
        match self.mode {
            RequestMode::SingleResponse => {
                let request = ClientRequest::new(self.history.clone())
                    .with_system(self.instructions.clone())
                    .with_prompt_cache_key(self.prompt_cache_key.clone())
                    .with_purpose(self.purpose)
                    .with_output_limit(self.limits.response().min(4096));
                match estimated_tokens(&request)? <= self.limits.input() {
                    true => Ok(BudgetPlan::Ready(request)),
                    false => Err(anyhow::anyhow!("Summary input exceeds its context budget")),
                }
            }
            RequestMode::Continue | RequestMode::Compact => self.conversation_plan(),
        }
    }

    fn conversation_plan(&self) -> anyhow::Result<BudgetPlan> {
        let exchanges = CompleteHistory::new(&self.history)?;
        let request = self.request(&self.checkpoint)?;
        match self.mode {
            RequestMode::Continue if estimated_tokens(&request)? < self.limits.trigger() => {
                Ok(BudgetPlan::Ready(request))
            }
            _ => CompactionPlan::new(self, &exchanges).map(BudgetPlan::Compact),
        }
    }

    fn prefix(&self, checkpoint: &Checkpoint, end: usize) -> anyhow::Result<Vec<Message>> {
        let remaining = self
            .history
            .get(checkpoint.through..end)
            .ok_or_else(|| anyhow::anyhow!("Saved compaction boundary exceeds the transcript"))?;
        let mut messages = checkpoint
            .memory
            .iter()
            .map(Memory::message)
            .collect::<Vec<_>>();
        messages.extend(protected(&self.history, checkpoint.through)?);
        messages.extend(
            remaining
                .iter()
                .enumerate()
                .filter_map(|(offset, message)| {
                    let content = message
                        .content
                        .iter()
                        .filter(|block| match block {
                            ContentBlock::RuntimeUpdate(_) => {
                                checkpoint.through + offset >= checkpoint.runtime_from
                            }
                            _ => true,
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    (!content.is_empty()).then(|| Message {
                        role: message.role.clone(),
                        content,
                    })
                }),
        );
        Ok(messages)
    }

    pub fn request(&self, checkpoint: &Checkpoint) -> anyhow::Result<ClientRequest> {
        let mut messages = self.prefix(checkpoint, self.history.len())?;
        messages.extend(self.runtime_update(checkpoint));
        let mut request = ClientRequest::new(messages)
            .with_system(self.instructions.clone())
            .with_prompt_cache_key(self.prompt_cache_key.clone())
            .with_purpose(self.purpose)
            .with_tools(self.tools.clone())
            .with_output_limit(self.limits.response())
            .with_thinking();
        if let Some(workspace) = self.history.first() {
            let bytes = (self.limits.trigger() / 4).min(16 * 1024) / 6;
            let text = utils::text::preview(&workspace.text(), bytes);
            request.messages.insert(0, Message::new(text));
        }
        Ok(request)
    }

    pub fn runtime_update(&self, checkpoint: &Checkpoint) -> Option<Message> {
        let previous = self
            .history
            .iter()
            .skip(checkpoint.runtime_from)
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::RuntimeUpdate(update) => Some(update),
                _ => None,
            })
            .fold(
                None,
                |state: Option<clients::runtime_update::RuntimeSnapshot>, update| {
                    Some(state.unwrap_or_default().apply(update))
                },
            );
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.update(previous.as_ref()))
            .map(|update| Message {
                role: Role::User,
                content: vec![ContentBlock::RuntimeUpdate(update)],
            })
    }

    pub fn compacted(&self, plan: &CompactionPlan, memory: Memory) -> anyhow::Result<Checkpoint> {
        let checkpoint = Checkpoint::new(
            &self.history,
            plan.through,
            self.checkpoint.generation + 1,
            memory,
        )?;
        let request = self.request(&checkpoint)?;
        let after = estimated_tokens(&request)?;
        let before = estimated_tokens(&self.request(&self.checkpoint)?)?;
        match after < before {
            true if after <= self.limits.trigger() => Ok(checkpoint),
            _ => Err(anyhow::anyhow!(
                "Compaction did not free enough context while preserving requirements and evidence. History is unchanged. Retry /compact or resume with a larger --context-tokens limit."
            )),
        }
    }
}

fn protected(history: &[Message], through: usize) -> anyhow::Result<Vec<Message>> {
    let prefix = history
        .get(1..through)
        .ok_or_else(|| anyhow::anyhow!("Invalid saved context boundary"))?;
    let requirements = prefix
        .iter()
        .filter(|message| matches!(message.role, Role::User))
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::MessageBlock { text, .. } => Some(Message::new(text.clone())),
            _ => None,
        });
    let mut calls = BTreeMap::new();
    let mut evidence = Vec::new();
    for block in prefix.iter().flat_map(|message| &message.content) {
        match block {
            ContentBlock::ToolBlock {
                tool_id,
                name,
                input,
            } => {
                calls.insert(
                    tool_id.id.as_ref(),
                    EvidenceCall {
                        name: name.as_ref(),
                        input,
                    },
                );
            }
            ContentBlock::ToolResult {
                tool_id,
                content,
                is_error,
            } => {
                if let Some(call) = calls.get(tool_id.id.as_ref()).filter(|call| {
                    matches!(
                        (call.name, is_error),
                        ("cargo" | "cargo_check" | "cargo_test" | "validate_rust", _)
                            | (_, Some(true))
                    )
                }) {
                    evidence.push(serde_json::json!({"tool": call.name, "input": call.input, "is_error": is_error, "result": content}));
                }
            }
            _ => {}
        }
    }
    let evidence = match evidence.is_empty() {
        true => None,
        false => Some(Message::new(format!(
            "Historical validation and failure evidence; results describe the workspace at execution time, not necessarily its current state:\n{}",
            serde_json::to_string(&evidence)?
        ))),
    };
    Ok(requirements.chain(evidence).collect())
}

struct EvidenceCall<'a> {
    name: &'a str,
    input: &'a serde_json::Map<String, serde_json::Value>,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub fn estimated_tokens(request: &ClientRequest) -> anyhow::Result<usize> {
    let tokenizer = tiktoken_rs::o200k_base_singleton();
    Ok(REQUEST_TOKEN_RESERVE
        + tokenizer.count_ordinary(request.system.as_deref().unwrap_or_default())
        + tokenizer.count_ordinary(&serde_json::to_string(&request.messages)?)
        + tokenizer.count_ordinary(&serde_json::to_string(&request.tools)?)
        + request.messages.len() * MESSAGE_TOKEN_RESERVE)
}
