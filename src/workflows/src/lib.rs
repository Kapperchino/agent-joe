pub mod continuation;
pub mod merge;
pub mod plan;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::Duration;
use tools::tool_defs::{LenientDeserialize, ToolOpKind};
use turbo_code_macros::ToolSchema;
use worker_registry::report::{WorkerReport, WorkerStatus};
use worker_registry::request::{WorkerRequest, WorkerRequestInput, WorkerRole};

const COMPLETION_CRITERIA: &str = "Complete the stated objective; report findings, requested versus executed validation, and every remaining limitation";
const CONSTRAINTS: &str = "Preserve all inherited constraints and unrelated user changes";

#[derive(Default, Debug, Clone, Serialize, Deserialize, ToolSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInput {
    #[serde(default)]
    #[tool(description = "Shared task context supplied to each step")]
    pub context: String,
    #[serde(default)]
    #[tool(
        description = "Reusable custom agents; built-in agents are gather_context, make_changes and validate_rust",
        max_items = 16
    )]
    pub agents: Vec<AgentInput>,
    #[tool(
        description = "Ordered steps; each agent step starts a separate bounded worker and receives prior step summaries",
        required,
        min_items = 1,
        max_items = 16
    )]
    pub steps: Vec<StepInput>,
}

impl WorkflowInput {
    pub fn single(agent: BuiltinAgent, objective: String) -> Self {
        let name = agent.definition().name;
        Self {
            steps: vec![StepInput::Agent {
                id: name.clone(),
                agent: name,
                objective,
                context: String::new(),
                completion_criteria: completion_criteria(),
            }],
            ..Default::default()
        }
    }
}

impl LenientDeserialize for WorkflowInput {
    fn deserialize_lenient(value: serde_json::Value) -> anyhow::Result<Self> {
        Ok(serde_json::from_value(value)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToolSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    #[tool(description = "Unique agent name", min_length = 1)]
    pub name: String,
    #[serde(default)]
    #[tool(
        description = "Agent specialization; inherited operating and user constraints still apply"
    )]
    pub instructions: String,
    #[tool(description = "Allowed non-delegating tool names separated by newlines")]
    pub allowed_tools: String,
    #[tool(
        description = "Allowed project-relative paths separated by newlines; . allows the project"
    )]
    pub allowed_paths: String,
    #[tool(
        description = "Per-agent deadline in seconds; default 1800",
        minimum = 1,
        maximum = 3600
    )]
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToolSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepInput {
    Agent {
        #[tool(min_length = 1)]
        id: String,
        #[tool(description = "Built-in or custom agent name", min_length = 1)]
        agent: String,
        #[tool(min_length = 1)]
        objective: String,
        #[serde(default)]
        context: String,
        #[serde(default = "completion_criteria")]
        completion_criteria: String,
    },
    Context {
        #[tool(min_length = 1)]
        id: String,
        #[tool(
            description = "Context added to subsequent agent handoffs",
            min_length = 1
        )]
        content: String,
    },
}

fn completion_criteria() -> String {
    COMPLETION_CRITERIA.into()
}

#[derive(Debug, Clone, Copy)]
pub enum BuiltinAgent {
    GatherContext,
    MakeChanges,
    ValidateRust,
}

impl BuiltinAgent {
    pub fn definition(self) -> AgentInput {
        let (name, tools) = match self {
            Self::GatherContext => (
                "gather_context",
                "find_files\nlist_directory\nknowledge\ngrep\ngit\nreview_changes",
            ),
            Self::MakeChanges => (
                "make_changes",
                "find_files\nlist_directory\nknowledge\ngrep\napply_patch\ncargo\ngit\nreview_changes\nundo_changes",
            ),
            Self::ValidateRust => ("validate_rust", "cargo"),
        };
        AgentInput {
            name: name.into(),
            instructions: CONSTRAINTS.into(),
            allowed_tools: tools.into(),
            allowed_paths: ".".into(),
            seconds: None,
        }
    }
}

pub struct Workflow {
    context: String,
    steps: Vec<Step>,
}

struct Step {
    id: String,
    action: StepAction,
}

enum StepAction {
    Agent(Box<WorkerRequest>),
    Context(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Completed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowReport {
    pub status: WorkflowStatus,
    pub steps: Vec<StepReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    pub id: String,
    pub output: StepOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepOutput {
    Agent {
        #[serde(deserialize_with = "deserialize_report")]
        report: Box<WorkerReport>,
    },
    Context {
        content: String,
    },
    Failed {
        message: String,
    },
    Skipped,
}

fn deserialize_report<'de, D>(deserializer: D) -> Result<Box<WorkerReport>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::from_value(serde_json::Value::deserialize(deserializer)?)
        .map(Box::new)
        .map_err(serde::de::Error::custom)
}

impl StepReport {
    fn handoff(&self) -> serde_json::Value {
        let output = match &self.output {
            StepOutput::Agent { report } => serde_json::json!({
                "worker_id": report.worker_id,
                "status": report.status,
                "findings": report.findings,
                "changed_files": report.changed_files,
                "possibly_changed_files": report.possibly_changed_files,
                "unresolved_issues": report.unresolved_issues,
                "artifacts": report.artifacts,
            }),
            StepOutput::Context { content } => serde_json::json!({"content": content}),
            StepOutput::Failed { message } => serde_json::json!({"error": message}),
            StepOutput::Skipped => serde_json::json!({"skipped": true}),
        };
        serde_json::json!({"id": self.id, "output": output})
    }
}

enum Progress {
    Running,
    Stopped,
}

impl StepOutput {
    fn progress(&self) -> Progress {
        match self {
            Self::Context { .. } => Progress::Running,
            Self::Agent { report } if report.status == WorkerStatus::Completed => Progress::Running,
            _ => Progress::Stopped,
        }
    }
}

impl Workflow {
    pub fn new(
        input: WorkflowInput,
        available: impl Fn(&str) -> Option<ToolOpKind>,
    ) -> anyhow::Result<Self> {
        match (
            input.steps.len(),
            input.agents.len(),
            serde_json::to_vec(&input)?.len(),
        ) {
            (1..=16, 0..=16, 0..=65536) => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Workflows require 1 to 16 steps, at most 16 custom agents and at most 64 KiB of configuration"
            )),
        }?;
        let mut agents = [
            BuiltinAgent::GatherContext,
            BuiltinAgent::MakeChanges,
            BuiltinAgent::ValidateRust,
        ]
        .into_iter()
        .map(BuiltinAgent::definition)
        .map(|agent| (agent.name.clone(), agent))
        .collect::<BTreeMap<_, _>>();
        for agent in input.agents {
            match (agent.name.trim(), agents.contains_key(&agent.name)) {
                ("", _) | (_, true) => Err(anyhow::anyhow!(
                    "Agent names must be nonempty and unique; built-in agents cannot be replaced"
                )),
                _ => Ok(()),
            }?;
            agents.insert(agent.name.clone(), agent);
        }
        let mut ids = BTreeSet::new();
        let steps = input
            .steps
            .into_iter()
            .map(|step| {
                let (id, action) = match step {
                    StepInput::Context { id, content } => {
                        match content.trim().is_empty() {
                            true => Err(anyhow::anyhow!("Context steps require nonempty content")),
                            false => Ok(()),
                        }?;
                        (id, StepAction::Context(content))
                    }
                    StepInput::Agent {
                        id,
                        agent,
                        objective,
                        context,
                        completion_criteria,
                    } => {
                        let definition = agents
                            .get(&agent)
                            .ok_or_else(|| anyhow::anyhow!("Unknown workflow agent `{agent}`"))?;
                        let request = WorkerRequest::new(
                            WorkerRequestInput {
                                objective,
                                constraints: format!("{CONSTRAINTS}\n{}", definition.instructions),
                                allowed_tools: definition.allowed_tools.clone(),
                                allowed_paths: definition.allowed_paths.clone(),
                                context,
                                completion_criteria,
                                seconds: definition.seconds,
                            },
                            &available,
                        )?;
                        (id, StepAction::Agent(Box::new(request)))
                    }
                };
                match (id.trim(), ids.insert(id.clone())) {
                    ("", _) | (_, false) => Err(anyhow::anyhow!(
                        "Workflow step IDs must be nonempty and unique"
                    )),
                    _ => Ok(Step { id, action }),
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            context: input.context,
            steps,
        })
    }

    pub fn single(
        agent: BuiltinAgent,
        objective: String,
        available: impl Fn(&str) -> Option<ToolOpKind>,
    ) -> anyhow::Result<Self> {
        Self::new(WorkflowInput::single(agent, objective), available)
    }

    pub fn effect(&self) -> ToolOpKind {
        match self.steps.iter().any(|step| matches!(&step.action, StepAction::Agent(request) if request.role() == WorkerRole::Write)) {
            true => ToolOpKind::DelegateWrite,
            false => ToolOpKind::DelegateRead,
        }
    }

    pub fn execution_budget(&self) -> Duration {
        self.steps
            .iter()
            .map(|step| match &step.action {
                StepAction::Agent(request) => Duration::from_secs(request.budget().seconds()),
                StepAction::Context(_) => Duration::ZERO,
            })
            .sum()
    }

    pub async fn run<F, Fut>(self, mut execute: F) -> WorkflowReport
    where
        F: FnMut(WorkerRequest) -> Fut,
        Fut: Future<Output = anyhow::Result<WorkerReport>>,
    {
        let mut progress = Progress::Running;
        let mut reports = Vec::<StepReport>::new();
        for step in self.steps {
            let output = match (&progress, step.action) {
                (Progress::Stopped, _) => StepOutput::Skipped,
                (Progress::Running, StepAction::Context(content)) => {
                    StepOutput::Context { content }
                }
                (Progress::Running, StepAction::Agent(request)) => {
                    let handoff = serde_json::json!({
                        "workflow_context": self.context,
                        "step_context": request.context(),
                        "previous_steps": reports.iter().map(StepReport::handoff).collect::<Vec<_>>(),
                    }).to_string();
                    let result = match request.with_context(handoff) {
                        Ok(request) => execute(request).await,
                        Err(error) => Err(error),
                    };
                    match result {
                        Ok(report) => StepOutput::Agent {
                            report: Box::new(report),
                        },
                        Err(error) => StepOutput::Failed {
                            message: format!("{error:#}"),
                        },
                    }
                }
            };
            progress = output.progress();
            reports.push(StepReport {
                id: step.id,
                output,
            });
        }
        WorkflowReport {
            status: match progress {
                Progress::Running => WorkflowStatus::Completed,
                Progress::Stopped => WorkflowStatus::Stopped,
            },
            steps: reports,
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/workflow/tests.rs"]
mod tests;
