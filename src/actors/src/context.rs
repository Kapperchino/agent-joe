use clients::{
    compaction::CompactedWindow,
    llm::{ClientRequest, ContentBlock, Message, Role},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use tools::tool_defs::ToolDefinition;

#[derive(Debug, Clone, Copy)]
pub struct ContextLimits {
    ceiling: usize,
    response: u32,
}

impl ContextLimits {
    pub fn new(ceiling: usize, response: u32) -> anyhow::Result<Self> {
        match ceiling >= 4096 && response >= 1024 && (response as usize) < ceiling / 2 {
            true => Ok(Self { ceiling, response }),
            false => Err(anyhow::anyhow!(
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
        self.input() - self.input() / 5
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
pub(crate) enum Memory {
    Summary(String),
    Native(CompactedWindow),
}

impl Memory {
    pub(crate) fn summary(text: String, limits: ContextLimits) -> anyhow::Result<Self> {
        match !text.trim().is_empty() && text.len() <= limits.summary_bytes() {
            true => Ok(Self::Summary(text)),
            false => Err(anyhow::anyhow!(
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
pub(crate) struct Checkpoint {
    pub through: usize,
    pub generation: u64,
    pub memory: Option<Memory>,
}

impl Default for Checkpoint {
    fn default() -> Self {
        Self {
            through: 1,
            generation: 0,
            memory: None,
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
        match through > 1 && exchanges.ends.contains(&through) {
            true => Ok(Self {
                through,
                generation,
                memory: Some(memory),
            }),
            false => Err(anyhow::anyhow!(
                "Compaction must end at a complete exchange"
            )),
        }
    }
}

pub(crate) struct CompleteHistory {
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
        let completes_message = matches!(message.role, Role::Assistant)
            || message
                .content
                .iter()
                .any(|content| matches!(content, ContentBlock::ToolResult { .. }));
        self.ends.extend(
            (matches!(self.exchange, ExchangeState::Complete) && completes_message)
                .then_some(index + 1),
        );
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
pub(crate) struct ContextInput {
    pub planning: Option<common_models::interaction::Planning>,
    pub history: Vec<Message>,
    pub checkpoint: Checkpoint,
    pub questions: Vec<crate::session::PendingQuestion>,
    pub instructions: String,
    pub tools: Vec<ToolDefinition>,
    pub limits: ContextLimits,
    pub native: NativeCompaction,
    pub mode: RequestMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestMode {
    Continue,
    Compact,
    SingleResponse,
}

pub(crate) enum BudgetPlan {
    Ready(ClientRequest),
    Compact(CompactionPlan),
}

pub(crate) struct CompactionPlan {
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
        match estimated_tokens(&request)? <= self.limits.trigger()
            && self.mode == RequestMode::Continue
        {
            true => Ok(BudgetPlan::Ready(request)),
            false => CompactionPlan::new(self, &exchanges).map(BudgetPlan::Compact),
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
        messages.extend_from_slice(remaining);
        Ok(messages)
    }

    pub fn request(&self, checkpoint: &Checkpoint) -> anyhow::Result<ClientRequest> {
        let mut messages = self.prefix(checkpoint, self.history.len())?;
        if let Some(planning) = &self.planning {
            messages.insert(0, Message::new(format!("Current planning state (runtime record; evidence source IDs may be cited by update_plan): {}", serde_json::to_string(planning)?)));
        }
        if !self.questions.is_empty() {
            messages.push(Message::new(format!(
                "Pending user questions (unanswered): {}",
                serde_json::to_string(&self.questions)?
            )));
        }
        let mut request = ClientRequest::new(messages)
            .with_system(self.instructions.clone())
            .with_tools(self.tools.clone())
            .with_output_limit(self.limits.response())
            .with_thinking();
        let essential = estimated_tokens(&request)?;
        let available = self
            .limits
            .trigger()
            .saturating_sub(essential)
            .saturating_sub(512)
            .min(16 * 1024);
        if available > 128
            && let Some(workspace) = self.history.first()
        {
            let text = crate::session::artifacts::preview(&workspace.text(), available / 6);
            request.messages.insert(0, Message::new(text));
        }
        Ok(request)
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
        match after <= self.limits.trigger() && after < before {
            true => Ok(checkpoint),
            false => Err(anyhow::anyhow!(
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
                if let Some(call) = calls.get(tool_id.id.as_ref())
                    && (*is_error == Some(true)
                        || matches!(
                            call.name,
                            "cargo" | "cargo_check" | "cargo_test" | "validate_rust"
                        ))
                {
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
#[path = "../tests/unit/context/tests.rs"]
mod tests;

pub(crate) fn estimated_tokens(request: &ClientRequest) -> anyhow::Result<usize> {
    let tokenizer = tiktoken_rs::o200k_base_singleton();
    Ok(1024
        + tokenizer.count_ordinary(request.system.as_deref().unwrap_or_default())
        + tokenizer.count_ordinary(&serde_json::to_string(&request.messages)?)
        + tokenizer.count_ordinary(&serde_json::to_string(&request.tools)?)
        + request.messages.len() * 32)
}
