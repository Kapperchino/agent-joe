pub use commands::command::Answer;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkMode {
    Plan,
    #[default]
    Implement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionInput {
    pub id: String,
    pub prompt: String,
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default = "allow_text")]
    pub allow_free_text: bool,
}

fn allow_text() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "QuestionInput")]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub required: bool,
    pub choices: Vec<Choice>,
    pub allow_free_text: bool,
}

impl TryFrom<QuestionInput> for Question {
    type Error = anyhow::Error;

    fn try_from(input: QuestionInput) -> anyhow::Result<Self> {
        let unique = input
            .choices
            .iter()
            .map(|choice| &choice.id)
            .collect::<BTreeSet<_>>();
        match valid_id(&input.id)
            && bounded_text(&input.prompt, 2048)
            && input.choices.len() <= 6
            && unique.len() == input.choices.len()
            && input
                .choices
                .iter()
                .all(|choice| valid_id(&choice.id) && bounded_text(&choice.label, 256))
            && (input.allow_free_text || !input.choices.is_empty())
        {
            true => Ok(Self {
                id: input.id,
                prompt: input.prompt,
                required: input.required,
                choices: input.choices,
                allow_free_text: input.allow_free_text,
            }),
            false => Err(anyhow::anyhow!(
                "Question needs a literal ID, a prompt of at most 2048 bytes, and at most six unique choices or free text"
            )),
        }
    }
}

impl Question {
    pub fn answer(&self, answer: &Answer) -> anyhow::Result<String> {
        match answer {
            Answer::Choice { choice_id } => self
                .choices
                .iter()
                .find(|choice| choice.id == *choice_id)
                .map(|choice| choice.label.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("Unknown choice {choice_id} for question {}", self.id)
                }),
            Answer::Text(text) if self.allow_free_text && bounded_text(text, 8192) => {
                Ok(text.clone())
            }
            Answer::Text(_) => Err(anyhow::anyhow!(
                "This question requires a listed choice or nonempty permitted text of at most 8192 bytes"
            )),
        }
    }

    pub fn display(&self) -> String {
        let kind = match self.required {
            true => "required",
            false => "optional",
        };
        let choices = self
            .choices
            .iter()
            .map(|choice| format!("{}: {}", choice.id, choice.label))
            .collect::<Vec<_>>()
            .join(" | ");
        let text = match self.allow_free_text {
            true => format!("\nFree text: /answer {} text <answer>.", self.id),
            false => String::new(),
        };
        let choices = match self.choices.is_empty() {
            true => String::new(),
            false => format!("\n{choices}\nChoose with /answer {} choice <id>.", self.id),
        };
        format!(
            "Question {} ({kind}): {}{choices}{text}",
            self.id, self.prompt
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEvidence {
    pub source: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub acceptance: String,
    pub state: StepState,
    #[serde(default)]
    pub evidence: Vec<PlanEvidence>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub revision: u64,
    pub requirements_revision: u64,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanUpdate {
    pub revision: u64,
    pub requirements_revision: u64,
    pub steps: Vec<PlanStep>,
}

impl Plan {
    pub fn update(
        &self,
        update: PlanUpdate,
        requirements_revision: u64,
        evidence: &BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        let steps = update
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step))
            .collect::<BTreeMap<_, _>>();
        let shape = !steps.is_empty()
            && steps.len() <= 16
            && steps.len() == update.steps.len()
            && update
                .steps
                .iter()
                .filter(|step| step.state == StepState::InProgress)
                .count()
                <= 1
            && update.steps.iter().all(|step| {
                valid_id(&step.id)
                    && bounded_text(&step.description, 512)
                    && bounded_text(&step.acceptance, 1024)
                    && step.dependencies.len() <= 15
                    && step.dependencies.iter().collect::<BTreeSet<_>>().len()
                        == step.dependencies.len()
                    && step
                        .dependencies
                        .iter()
                        .all(|id| steps.contains_key(id.as_str()) && *id != step.id)
                    && step.evidence.len() <= 8
                    && step.evidence.iter().all(|item| {
                        evidence.contains_key(&item.source) && bounded_text(&item.explanation, 512)
                    })
                    && match (&step.state, &step.blocked_reason) {
                        (StepState::Blocked, Some(reason)) => bounded_text(reason, 1024),
                        (StepState::Blocked, None) => false,
                        (_, None) => true,
                        (_, Some(_)) => false,
                    }
            });
        match shape
            && update.revision == self.revision
            && update.requirements_revision == requirements_revision
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Invalid or stale plan: use current revisions, 1–16 unique bounded steps, valid dependencies and recorded evidence"
            )),
        }?;
        let mut reachable = BTreeSet::new();
        for _ in 0..steps.len() {
            let next = steps
                .values()
                .filter(|step| {
                    step.dependencies
                        .iter()
                        .all(|id| reachable.contains(id.as_str()))
                })
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>();
            reachable.extend(next);
        }
        match reachable.len() == steps.len() {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Plan dependencies contain a cycle")),
        }?;
        for step in &update.steps {
            let previous = self.steps.iter().find(|previous| previous.id == step.id);
            let definition_unchanged = previous.is_some_and(|previous| {
                previous.description == step.description
                    && previous.acceptance == step.acceptance
                    && previous.dependencies == step.dependencies
            });
            let transition = match (previous.map(|step| step.state), step.state) {
                (None, StepState::Pending | StepState::Blocked) => true,
                (Some(StepState::Pending | StepState::Blocked), StepState::InProgress) => true,
                (Some(StepState::InProgress), StepState::Completed) => definition_unchanged,
                (Some(_), StepState::Pending | StepState::Blocked) => true,
                (Some(old), new) if old == new => definition_unchanged || new == StepState::Pending,
                _ => false,
            };
            let ready = step
                .dependencies
                .iter()
                .all(|id| steps[id.as_str()].state == StepState::Completed);
            let supported = match step.state {
                StepState::Completed => ready && !step.evidence.is_empty(),
                StepState::InProgress => ready,
                StepState::Pending | StepState::Blocked => true,
            };
            let reconciled = self.requirements_revision == requirements_revision
                || step.state != StepState::Completed;
            match transition && supported && reconciled {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Step {} has an unsupported transition: start before completing, satisfy dependencies, attach successful evidence, and reopen completed steps after requirements change",
                    step.id
                )),
            }?;
        }
        match self
            .steps
            .iter()
            .all(|step| step.state != StepState::Completed || steps.contains_key(step.id.as_str()))
        {
            true => Ok(Self {
                revision: self
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Plan revision exhausted"))?,
                requirements_revision,
                steps: update.steps,
            }),
            false => Err(anyhow::anyhow!(
                "Reopen a completed step before removing it"
            )),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Planning {
    pub mode: WorkMode,
    pub plan: Plan,
    pub requirements_revision: u64,
    pub evidence: BTreeMap<String, String>,
}

impl Planning {
    pub fn reconcile(&mut self) -> anyhow::Result<()> {
        self.requirements_revision = self
            .requirements_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Requirements revision exhausted"))?;
        Ok(())
    }

    pub fn record_evidence(&mut self, id: String, detail: String) {
        self.evidence.insert(id, detail.chars().take(256).collect());
        let retained = self
            .plan
            .steps
            .iter()
            .flat_map(|step| step.evidence.iter().map(|item| item.source.as_str()))
            .collect::<BTreeSet<_>>();
        let excess = self.evidence.len().saturating_sub(256);
        let evicted = self
            .evidence
            .keys()
            .filter(|id| !retained.contains(id.as_str()))
            .take(excess)
            .cloned()
            .collect::<Vec<_>>();
        for id in evicted {
            self.evidence.remove(&id);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct InteractionView {
    pub planning: Planning,
    pub questions: Vec<Question>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn bounded_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty()
        && text.len() <= max
        && !text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
}
