use crate::{BuiltinAgent, StepInput, WorkflowInput};
use clients::llm::{Message, Role};
use common_models::interaction::{
    Answer, Choice, Investigation, PlanReview, Planning, Question, QuestionInput, QuestionPurpose,
    WorkMode,
};
use common_models::runtime_ids::TurnId;

pub enum PlanContinuation {
    Implement,
    NewAgent,
    KeepPlanning,
}

impl TryFrom<&Answer> for PlanContinuation {
    type Error = anyhow::Error;

    fn try_from(answer: &Answer) -> anyhow::Result<Self> {
        match answer {
            Answer::Choice { choice_id } => match choice_id.as_str() {
                "implement" => Ok(Self::Implement),
                "new_agent" => Ok(Self::NewAgent),
                "keep_planning" => Ok(Self::KeepPlanning),
                _ => Err(anyhow::anyhow!("Unknown plan continuation choice")),
            },
            Answer::Text(_) => Err(anyhow::anyhow!("Choose a listed plan continuation option")),
        }
    }
}

pub struct PlanHandoff {
    pub planning: Planning,
    pub prompt: String,
}

pub struct PlanContext<'a> {
    pub planning: &'a Planning,
    pub history: &'a [Message],
    pub requirements: String,
}

impl PlanHandoff {
    pub fn new(context: PlanContext<'_>) -> anyhow::Result<Self> {
        let planning = context.planning;
        match (
            planning.mode,
            planning.review(),
            planning.plan.investigation(),
        ) {
            (WorkMode::Plan, PlanReview::Current, Investigation::Complete) => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Finish and reconcile the planning investigation before implementing it"
            )),
        }?;
        let plan = context
            .history
            .iter()
            .rev()
            .find(|message| matches!(message.role, Role::Assistant) && !message.text().is_empty())
            .map(|message| message.text())
            .ok_or_else(|| {
                anyhow::anyhow!("The completed plan is missing from the conversation")
            })?;
        let requirements = context.requirements;
        Ok(Self {
            planning: Planning {
                mode: WorkMode::Implement,
                ..planning.clone()
            },
            prompt: format!(
                "Implement the plan approved by the user below. Preserve its requirements and decisions, continue the tracked implementation steps, and run the planned validation. Investigation evidence is historical context, not proof of implementation or validation in this session; re-read source files as needed. Artifacts from the planning session may not be available here.\n\nPlanning requirements and user answers:\n{requirements}\n\nApproved plan:\n{plan}"
            ),
        })
    }

    pub fn workflow(&self) -> anyhow::Result<WorkflowInput> {
        let mut input = WorkflowInput::single(
            BuiltinAgent::MakeChanges,
            "Implement the approved plan. Preserve its requirements and decisions, add focused regression coverage, and report implementation evidence and any blockers. The root coordinator owns tracked-plan updates.".into(),
        );
        input.context = format!(
            "{}\n\nTracked plan and exact requested validation:\n{}",
            self.prompt,
            serde_json::to_string(&self.planning)?
        );
        input.steps.push(StepInput::Agent {
            id: "validate_rust".into(),
            agent: "validate_rust".into(),
            objective: "Run the approved plan's exact requested Cargo checks against the current workspace, then relevant focused regression checks. Assess the previous implementation report. Report actual results, failures and checks that could not run; do not infer success from compilation.".into(),
            context: String::new(),
            completion_criteria: "Every requested check has observed evidence, or is explicitly reported as blocked or failed. Do not claim completion when required validation fails.".into(),
        });
        Ok(input)
    }

    pub fn question(turn: TurnId) -> anyhow::Result<Question> {
        QuestionInput {
            purpose: QuestionPurpose::PlanContinuation,
            id: format!("plan-{turn}"),
            prompt: "The plan is ready. Would you like to implement it in this session or start a new agent with the plan?".into(),
            required: false,
            choices: [
                ("implement", "Implement in this session"),
                ("new_agent", "Start a new agent with this plan"),
                ("keep_planning", "Keep planning"),
            ]
            .into_iter()
            .map(|(id, label)| Choice { id: id.into(), label: label.into() })
            .collect(),
            allow_free_text: false,
        }
        .try_into()
    }
}
