use super::request::BudgetLimits;
use std::sync::Mutex;

pub struct WorkerBudget {
    limits: BudgetLimits,
    usage: Mutex<BudgetUsage>,
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
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            usage: Mutex::new(BudgetUsage::default()),
        }
    }

    pub fn usage(&self) -> BudgetUsage {
        self.usage.lock().unwrap().clone()
    }

    pub fn reserve(&self, request: &mut clients::llm::ClientRequest) -> anyhow::Result<()> {
        let mut usage = self.usage.lock().unwrap();
        let input = crate::context::estimated_tokens(request)?;
        let charged = usage.reserved_tokens.max(
            usage
                .reported_input_tokens
                .saturating_add(usage.reported_output_tokens),
        );
        let remaining = self.limits.tokens().saturating_sub(charged);
        let output = remaining.saturating_sub(input).min(4096) as u32;
        match usage.state {
            BudgetState::Available if usage.requests < self.limits.requests() && output >= 256 => {
                let output = request.max_output_tokens.unwrap_or(output).min(output);
                request.max_output_tokens = Some(output);
                usage.reserved_tokens = charged + input + output as usize;
                usage.requests += 1;
                Ok(())
            }
            _ => Err(usage.exhausted("Worker token or request budget exhausted")),
        }
    }

    pub fn observe(&self, event: &clients::llm::StreamEvent) -> anyhow::Result<()> {
        let mut usage = self.usage.lock().unwrap();
        match event {
            clients::llm::StreamEvent::MessageStart { message } => {
                usage.reported_input_tokens = usage
                    .reported_input_tokens
                    .saturating_add(message.usage.input_tokens as usize)
            }
            clients::llm::StreamEvent::MessageDelta {
                usage: reported, ..
            } => {
                usage.reported_input_tokens = usage
                    .reported_input_tokens
                    .saturating_add(reported.input_tokens as usize);
                usage.reported_output_tokens = usage
                    .reported_output_tokens
                    .saturating_add(reported.output_tokens as usize);
            }
            _ => {}
        }
        match usage.state {
            BudgetState::Available
                if usage
                    .reported_input_tokens
                    .saturating_add(usage.reported_output_tokens)
                    <= self.limits.tokens() =>
            {
                Ok(())
            }
            _ => Err(usage.exhausted("Provider-reported worker token budget exhausted")),
        }
    }

    pub fn tool_call(&self) -> anyhow::Result<()> {
        let mut usage = self.usage.lock().unwrap();
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
mod tests {
    use super::*;
    use clients::llm::{ClientRequest, Message, MessageDeltaContent, StreamEvent, UsageDelta};

    #[test]
    fn exhausted_budgets_reject_further_requests_events_and_tools() {
        let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
        let mut request = ClientRequest::new(vec![Message::new("Bounded request".into())]);
        budget.reserve(&mut request).unwrap();
        assert!(budget.reserve(&mut request).is_err());
        assert!(budget.tool_call().is_err());
        assert!(budget.observe(&StreamEvent::Ping).is_err());
        assert_eq!(budget.usage().requests, 1);
        assert_eq!(budget.usage().tool_calls, 0);
        assert_eq!(budget.usage().state, BudgetState::Exhausted);
        let encoded = serde_json::to_value(budget.usage()).unwrap();
        assert_eq!(encoded["exhausted"], true);
        let restored: BudgetUsage = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.state, BudgetState::Exhausted);

        let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
        let over_limit = StreamEvent::MessageDelta {
            delta: MessageDeltaContent { stop_reason: None },
            usage: UsageDelta {
                input_tokens: 4096,
                output_tokens: 1,
            },
        };
        assert!(budget.observe(&over_limit).is_err());
        assert!(budget.tool_call().is_err());
        assert!(budget.reserve(&mut request).is_err());
        assert_eq!(budget.usage().reported_output_tokens, 1);
        assert_eq!(budget.usage().state, BudgetState::Exhausted);

        let budget = WorkerBudget::new(BudgetLimits::new(4096, 1, 1).unwrap());
        for _ in 0..128 {
            budget.tool_call().unwrap();
        }
        assert!(budget.tool_call().is_err());
        assert!(budget.reserve(&mut request).is_err());
        assert_eq!(budget.usage().tool_calls, 128);
        assert_eq!(budget.usage().state, BudgetState::Exhausted);
    }
}
