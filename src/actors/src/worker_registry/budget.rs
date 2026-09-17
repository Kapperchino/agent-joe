use std::sync::Mutex;

#[derive(Default)]
pub struct WorkerBudget {
    ledger: Mutex<BudgetLedger>,
}

#[derive(Default)]
struct BudgetLedger {
    usage: BudgetUsage,
    request: RequestState,
}

#[derive(Default)]
enum RequestState {
    #[default]
    Idle,
    Reserved(RequestUsage),
}

struct RequestUsage {
    reserved_tokens: usize,
    estimated_input_tokens: usize,
    input_tokens: usize,
    output_tokens: usize,
}

impl BudgetLedger {
    fn observe(&mut self, event: &clients::llm::StreamEvent) {
        if let RequestState::Reserved(request) = &mut self.request {
            let previous_input = request.input_tokens;
            let previous_output = request.output_tokens;
            match event {
                clients::llm::StreamEvent::MessageStart { message } => {
                    let input = (message.usage.input_tokens as usize)
                        .saturating_add(message.usage.cache_creation_input_tokens as usize)
                        .saturating_add(message.usage.cache_read_input_tokens as usize);
                    request.input_tokens = request.input_tokens.max(input);
                    request.output_tokens = request
                        .output_tokens
                        .max(message.usage.output_tokens as usize);
                }
                clients::llm::StreamEvent::MessageDelta { usage, .. } => {
                    request.input_tokens = request.input_tokens.max(usage.input_tokens as usize);
                    request.output_tokens = request.output_tokens.max(usage.output_tokens as usize);
                }
                _ => {}
            }
            self.usage.reported_input_tokens = self
                .usage
                .reported_input_tokens
                .saturating_add(request.input_tokens - previous_input);
            self.usage.reported_output_tokens = self
                .usage
                .reported_output_tokens
                .saturating_add(request.output_tokens - previous_output);
            let completed = matches!(event, clients::llm::StreamEvent::MessageDelta { delta, .. } if delta.stop_reason.is_some());
            if completed && (request.input_tokens > 0 || request.output_tokens > 0) {
                let input = match request.input_tokens {
                    0 => request.estimated_input_tokens,
                    input => input,
                };
                self.usage.reserved_tokens = self
                    .usage
                    .reserved_tokens
                    .saturating_sub(request.reserved_tokens)
                    .saturating_add(input)
                    .saturating_add(request.output_tokens);
            }
            if completed {
                self.request = RequestState::Idle;
            }
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BudgetUsage {
    pub reserved_tokens: usize,
    pub requests: usize,
    pub tool_calls: usize,
    #[serde(rename = "exhausted")]
    pub state: BudgetState,
    pub reported_input_tokens: usize,
    pub reported_output_tokens: usize,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(from = "bool", into = "bool")]
pub enum BudgetState {
    #[default]
    Available,
    Exhausted,
}

impl From<bool> for BudgetState {
    fn from(exhausted: bool) -> Self {
        match exhausted {
            true => Self::Exhausted,
            false => Self::Available,
        }
    }
}

impl From<BudgetState> for bool {
    fn from(state: BudgetState) -> Self {
        matches!(state, BudgetState::Exhausted)
    }
}

impl BudgetUsage {
    fn exhausted(&mut self, message: &str) -> anyhow::Error {
        self.state = BudgetState::Exhausted;
        anyhow::anyhow!("{message}")
    }
}

impl WorkerBudget {
    pub fn usage(&self) -> BudgetUsage {
        self.ledger.lock().unwrap().usage.clone()
    }

    pub fn reserve(&self, request: &clients::llm::ClientRequest) -> anyhow::Result<()> {
        let mut ledger = self.ledger.lock().unwrap();
        let usage = &mut ledger.usage;
        let input = crate::context::estimated_tokens(request)?;
        let charged = usage.reserved_tokens.max(
            usage
                .reported_input_tokens
                .saturating_add(usage.reported_output_tokens),
        );
        let output = request.max_output_tokens.unwrap_or_default() as usize;
        match usage.state {
            BudgetState::Exhausted => Err(anyhow::anyhow!("Worker budget already exhausted")),
            BudgetState::Available => {
                usage.reserved_tokens = charged.saturating_add(input).saturating_add(output);
                usage.requests = usage.requests.saturating_add(1);
                ledger.request = RequestState::Reserved(RequestUsage {
                    reserved_tokens: input + output,
                    estimated_input_tokens: input,
                    input_tokens: 0,
                    output_tokens: 0,
                });
                Ok(())
            }
        }
    }

    pub fn observe(&self, event: &clients::llm::StreamEvent) -> anyhow::Result<()> {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.observe(event);
        let usage = &mut ledger.usage;
        match usage.state {
            BudgetState::Available => Ok(()),
            BudgetState::Exhausted => Err(anyhow::anyhow!("Worker budget already exhausted")),
        }
    }

    pub fn tool_call(&self) -> anyhow::Result<()> {
        let mut ledger = self.ledger.lock().unwrap();
        let usage = &mut ledger.usage;
        match usage.state {
            BudgetState::Available if usage.tool_calls < 128 => {
                usage.tool_calls += 1;
                Ok(())
            }
            _ => Err(usage.exhausted("Worker tool-call budget exhausted (128 calls)")),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/worker_registry/budget/tests.rs"]
mod tests;
