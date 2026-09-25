use crate::context::{CompleteHistory, ContextLimits, estimated_tokens};
use clients::llm::{ClientRequest, Message, RequestPurpose};

pub const QUESTION_INSTRUCTIONS: &str = "Answer only the question in the next message using the preceding context. Do not continue earlier tasks or call tools. If the context is insufficient, say so. This question and its answer will not be retained.";
pub const MAX_QUESTION_BYTES: usize = 16 * 1024;
const QUESTION_OVERHEAD_TOKENS: usize = 512;
const JSON_BYTES_PER_INPUT_BYTE: usize = 6;

#[derive(Clone, Copy)]
pub struct SnapshotBudget {
    limits: ContextLimits,
}

impl SnapshotBudget {
    pub fn new(limits: ContextLimits) -> Self {
        Self { limits }
    }

    pub fn question_bytes(self) -> usize {
        (self.limits.input() / 64).min(MAX_QUESTION_BYTES)
    }

    pub fn context_tokens(self) -> usize {
        self.limits.input().saturating_sub(
            self.question_bytes() * JSON_BYTES_PER_INPUT_BYTE + QUESTION_OVERHEAD_TOKENS,
        )
    }
}

pub struct FrozenContext {
    request: ClientRequest,
    budget: SnapshotBudget,
}

impl FrozenContext {
    pub fn new(request: ClientRequest, limits: ContextLimits) -> anyhow::Result<Self> {
        match request.messages.is_empty() {
            true => Err(anyhow::anyhow!("A snapshot requires captured context")),
            false => Ok(()),
        }?;
        let history = std::iter::once(Message::new(String::new()))
            .chain(request.messages.iter().cloned())
            .collect::<Vec<_>>();
        CompleteHistory::new(&history)?;
        let budget = SnapshotBudget::new(limits);
        let required = estimated_tokens(&request)?;
        match required <= budget.context_tokens() {
            true => Ok(Self {
                request: ClientRequest {
                    max_output_tokens: Some(limits.response()),
                    ..request
                },
                budget,
            }),
            false => Err(anyhow::anyhow!(
                "Snapshot context requires {required} tokens but only {} are available after reserving space for a {}-byte question and the answer; context was not truncated or compacted",
                budget.context_tokens(),
                budget.question_bytes(),
            )),
        }
    }

    pub fn max_question_bytes(&self) -> usize {
        self.budget.question_bytes()
    }

    pub fn request(&self) -> &ClientRequest {
        &self.request
    }

    pub fn question_request(&self, question: String) -> anyhow::Result<ClientRequest> {
        let question = SnapshotQuestion::new(question, self.budget)?;
        let mut request = self.request.clone();
        request.messages.extend([
            Message::new(QUESTION_INSTRUCTIONS.into()),
            Message::new(question.0),
        ]);
        request.purpose = RequestPurpose::Worker;
        Ok(request)
    }
}

struct SnapshotQuestion(String);

impl SnapshotQuestion {
    fn new(question: String, budget: SnapshotBudget) -> anyhow::Result<Self> {
        match (question.trim().is_empty(), question.len()) {
            (true, _) => Err(anyhow::anyhow!("A snapshot question must not be empty")),
            (false, bytes) if bytes <= budget.question_bytes() => Ok(Self(question)),
            _ => Err(anyhow::anyhow!(
                "Snapshot questions are limited to {} UTF-8 bytes; use a shorter question",
                budget.question_bytes(),
            )),
        }
    }
}
