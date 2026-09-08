use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tools::tool_defs::ToolEffect;
use turbo_code_macros::ToolInput;

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolInput)]
pub struct WorkerRequestInput {
    #[tool(description = "A concrete bounded objective", required)]
    pub objective: String,
    #[tool(
        description = "Additional constraints; parent and user constraints are inherited automatically",
        required
    )]
    pub constraints: String,
    #[tool(description = "Allowed tool names separated by newlines", required)]
    pub allowed_tools: String,
    #[tool(
        description = "Allowed project-relative files or directories separated by newlines; . allows the project",
        required
    )]
    pub allowed_paths: String,
    #[tool(
        description = "Selected relevant context, file locations, and artifact references",
        required
    )]
    pub context: String,
    #[tool(
        description = "Observable completion criteria, including requested validation and limitations",
        required
    )]
    pub completion_criteria: String,
    #[tool(
        description = "Conservative total input/output token budget, 1024 to 500000; default 120000"
    )]
    pub tokens: Option<usize>,
    #[tool(description = "Wall-clock budget in seconds, 1 to 300; default 180")]
    pub seconds: Option<u64>,
    #[tool(description = "Maximum provider requests, 1 to 32; default 16")]
    pub requests: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRequest {
    objective: String,
    constraints: String,
    pub(super) allowed_tools: Vec<String>,
    pub(super) allowed_paths: Vec<PathBuf>,
    context: String,
    pub(super) completion_criteria: String,
    pub(super) budget: BudgetLimits,
    pub(super) role: WorkerRole,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "BudgetLimitsInput")]
pub struct BudgetLimits {
    tokens: usize,
    seconds: u64,
    requests: usize,
}

#[derive(Deserialize)]
struct BudgetLimitsInput {
    tokens: usize,
    seconds: u64,
    requests: usize,
}

impl TryFrom<BudgetLimitsInput> for BudgetLimits {
    type Error = anyhow::Error;

    fn try_from(input: BudgetLimitsInput) -> anyhow::Result<Self> {
        Self::new(input.tokens, input.seconds, input.requests)
    }
}

impl BudgetLimits {
    pub fn new(tokens: usize, seconds: u64, requests: usize) -> anyhow::Result<Self> {
        match (1024..=500_000).contains(&tokens)
            && (1..=300).contains(&seconds)
            && (1..=32).contains(&requests)
        {
            true => Ok(Self {
                tokens,
                seconds,
                requests,
            }),
            false => Err(anyhow::anyhow!(
                "Invalid worker budget: tokens must be 1024 to 500000, seconds 1 to 300, and requests 1 to 32"
            )),
        }
    }

    pub(super) fn tokens(self) -> usize {
        self.tokens
    }

    pub(super) fn seconds(self) -> u64 {
        self.seconds
    }

    pub(super) fn requests(self) -> usize {
        self.requests
    }
}

impl WorkerRequest {
    pub fn new(
        input: WorkerRequestInput,
        available: impl Fn(&str) -> Option<ToolEffect>,
    ) -> anyhow::Result<Self> {
        let encoded = serde_json::to_vec(&input)?;
        let tools = input
            .allowed_tools
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let paths = input
            .allowed_paths
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        let budget = BudgetLimits::new(
            input.tokens.unwrap_or(120_000),
            input.seconds.unwrap_or(180),
            input.requests.unwrap_or(16),
        )?;
        let valid = !input.objective.trim().is_empty()
            && !input.completion_criteria.trim().is_empty()
            && !tools.is_empty()
            && tools.len() <= 32
            && !paths.is_empty()
            && paths.len() <= 64
            && paths.iter().all(|path| {
                !path.is_absolute()
                    && !path
                        .components()
                        .any(|part| matches!(part, std::path::Component::ParentDir))
            })
            && encoded.len() <= 64 * 1024;
        let effects = tools.iter().map(|tool| {
            match available(tool) {
                Some(effect) if !effect.delegates() => Ok(effect),
                _ => Err(anyhow::anyhow!("Worker tool `{tool}` is unavailable or delegates; maximum delegation depth is one")),
            }
        }).collect::<anyhow::Result<Vec<_>>>()?;
        let role = match effects
            .iter()
            .any(|effect| matches!(effect, ToolEffect::Write | ToolEffect::Validate))
        {
            true => WorkerRole::Write,
            false => WorkerRole::Read,
        };
        match valid {
            true => Ok(Self {
                objective: input.objective,
                constraints: input.constraints,
                allowed_tools: tools,
                allowed_paths: paths,
                context: input.context,
                completion_criteria: input.completion_criteria,
                budget,
                role,
            }),
            false => Err(anyhow::anyhow!(
                "Invalid worker request: provide an objective, completion criteria and bounded tool/path lists (handoff limit 64 KiB)"
            )),
        }
    }

    pub(crate) fn allows_tool(&self, name: &str) -> bool {
        self.allowed_tools.iter().any(|tool| tool == name)
    }

    pub(crate) fn follow_up(
        &self,
        objective: String,
        context: String,
        available: impl Fn(&str) -> Option<ToolEffect>,
    ) -> anyhow::Result<Self> {
        Self::new(
            WorkerRequestInput {
                objective,
                constraints: self.constraints.clone(),
                allowed_tools: self.allowed_tools.join("\n"),
                allowed_paths: self
                    .allowed_paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("\n"),
                context,
                completion_criteria: self.completion_criteria.clone(),
                tokens: Some(self.budget.tokens()),
                seconds: Some(self.budget.seconds()),
                requests: Some(self.budget.requests()),
            },
            available,
        )
    }

    pub(crate) fn prompt(&self, inherited: &[String]) -> anyhow::Result<String> {
        let prompt = format!(
            "Inherited parent/user requirements (preserve all):\n{}\n\nBounded worker request:\n{}",
            inherited.join("\n\n"),
            serde_json::to_string_pretty(self)?
        );
        match prompt.len() <= 128 * 1024 {
            true => Ok(prompt),
            false => Err(anyhow::anyhow!(
                "Worker handoff exceeds 128 KiB; required parent constraints cannot be truncated"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn followups_recheck_handoff_bounds_and_preserve_the_original_constraints() {
        let request = WorkerRequest::new(
            WorkerRequestInput {
                objective: "Inspect selected files".into(),
                constraints: "Preserve public APIs".into(),
                allowed_tools: "read_file".into(),
                allowed_paths: "src".into(),
                completion_criteria: "Report evidence".into(),
                ..Default::default()
            },
            |_| Some(ToolEffect::Read),
        )
        .unwrap();
        let followup = request
            .follow_up("Confirm finding".into(), "Selected report".into(), |_| {
                Some(ToolEffect::Read)
            })
            .unwrap();
        assert_eq!(followup.constraints, request.constraints);
        assert_eq!(followup.allowed_tools, request.allowed_tools);
        assert_eq!(followup.allowed_paths, request.allowed_paths);
        assert_eq!(followup.completion_criteria, request.completion_criteria);
        assert_eq!(followup.budget.tokens(), request.budget.tokens());
        assert_eq!(followup.budget.seconds(), request.budget.seconds());
        assert_eq!(followup.budget.requests(), request.budget.requests());
        assert!(
            request
                .follow_up("Confirm finding".into(), "x".repeat(64 * 1024), |_| Some(
                    ToolEffect::Read
                ))
                .is_err()
        );
        assert!(
            request
                .follow_up(String::new(), String::new(), |_| Some(ToolEffect::Read))
                .is_err()
        );
        assert!(
            request
                .follow_up("Confirm finding".into(), String::new(), |_| None)
                .is_err()
        );
    }

    #[test]
    fn deserialized_budgets_cannot_bypass_constructor_limits() {
        for input in [
            serde_json::json!({"tokens": 1023, "seconds": 1, "requests": 1}),
            serde_json::json!({"tokens": 500001, "seconds": 1, "requests": 1}),
            serde_json::json!({"tokens": 1024, "seconds": 0, "requests": 1}),
            serde_json::json!({"tokens": 1024, "seconds": 301, "requests": 1}),
            serde_json::json!({"tokens": 1024, "seconds": 1, "requests": 0}),
            serde_json::json!({"tokens": 1024, "seconds": 1, "requests": 33}),
        ] {
            assert!(serde_json::from_value::<BudgetLimits>(input).is_err());
        }
        let limits = BudgetLimits::new(500_000, 300, 32).unwrap();
        let restored: BudgetLimits =
            serde_json::from_value(serde_json::to_value(limits).unwrap()).unwrap();
        assert_eq!(restored.tokens(), limits.tokens());
        assert_eq!(restored.seconds(), limits.seconds());
        assert_eq!(restored.requests(), limits.requests());
    }
}
