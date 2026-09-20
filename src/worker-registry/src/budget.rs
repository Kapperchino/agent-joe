use std::sync::Mutex;

#[derive(Default)]
pub struct WorkerBudget {
    ledger: Mutex<BudgetLedger>,
}

#[derive(Debug, Clone, Copy)]
pub struct RequestReservation {
    pub estimated_input_tokens: usize,
    pub output_tokens: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
}

pub enum UsageUpdate {
    Unreported,
    Progress(TokenUsage),
    Completed(TokenUsage),
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

impl RequestUsage {
    fn charge(&self) -> usize {
        match (self.input_tokens, self.output_tokens) {
            (0, 0) => self.reserved_tokens,
            (0, output) => self.estimated_input_tokens.saturating_add(output),
            (input, output) => input.saturating_add(output),
        }
    }
}

impl BudgetLedger {
    fn observe(&mut self, update: UsageUpdate) {
        match (&mut self.request, &update) {
            (
                RequestState::Reserved(request),
                UsageUpdate::Progress(tokens) | UsageUpdate::Completed(tokens),
            ) => {
                let previous_input = request.input_tokens;
                let previous_output = request.output_tokens;
                request.input_tokens = request.input_tokens.max(tokens.input_tokens);
                request.output_tokens = request.output_tokens.max(tokens.output_tokens);
                self.usage.reported_input_tokens = self
                    .usage
                    .reported_input_tokens
                    .saturating_add(request.input_tokens - previous_input);
                self.usage.reported_output_tokens = self
                    .usage
                    .reported_output_tokens
                    .saturating_add(request.output_tokens - previous_output);
                if let UsageUpdate::Completed(_) = update {
                    self.usage.reserved_tokens = self
                        .usage
                        .reserved_tokens
                        .saturating_sub(request.reserved_tokens)
                        .saturating_add(request.charge());
                    self.request = RequestState::Idle;
                }
            }
            _ => {}
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BudgetState {
    #[default]
    Available,
    Exhausted,
}

impl serde::Serialize for BudgetState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(matches!(self, Self::Exhausted))
    }
}

impl<'de> serde::Deserialize<'de> for BudgetState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match bool::deserialize(deserializer)? {
            true => Self::Exhausted,
            false => Self::Available,
        })
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

    pub fn reserve(&self, reservation: RequestReservation) -> anyhow::Result<()> {
        let mut ledger = self.ledger.lock().unwrap();
        let usage = &mut ledger.usage;
        let charged = usage.reserved_tokens.max(
            usage
                .reported_input_tokens
                .saturating_add(usage.reported_output_tokens),
        );
        match usage.state {
            BudgetState::Exhausted => Err(anyhow::anyhow!("Worker budget already exhausted")),
            BudgetState::Available => {
                let reserved_tokens = reservation
                    .estimated_input_tokens
                    .saturating_add(reservation.output_tokens);
                usage.reserved_tokens = charged.saturating_add(reserved_tokens);
                usage.requests = usage.requests.saturating_add(1);
                ledger.request = RequestState::Reserved(RequestUsage {
                    reserved_tokens,
                    estimated_input_tokens: reservation.estimated_input_tokens,
                    input_tokens: 0,
                    output_tokens: 0,
                });
                Ok(())
            }
        }
    }

    pub fn observe(&self, update: UsageUpdate) -> anyhow::Result<()> {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.observe(update);
        match ledger.usage.state {
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
#[path = "../tests/unit/budget/tests.rs"]
mod tests;
