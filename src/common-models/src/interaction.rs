mod plan;
mod question;

pub use plan::{Plan, PlanEvidence, PlanStep, PlanUpdate, StepState};
pub use question::{
    Answer, AnsweredQuestion, Choice, Question, QuestionGate, QuestionInput, Questions,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkMode {
    Plan,
    #[default]
    Implement,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanReview {
    #[default]
    Current,
    Required,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Planning {
    pub mode: WorkMode,
    pub plan: Plan,
    pub requirements_revision: u64,
    pub evidence: BTreeMap<String, String>,
}

impl Planning {
    pub fn review(&self) -> PlanReview {
        match self.plan.requirements_revision == self.requirements_revision {
            true => PlanReview::Current,
            false => PlanReview::Required,
        }
    }

    pub fn requirements_changed(&self) -> anyhow::Result<Self> {
        let requirements_revision = match self.plan.steps.is_empty() {
            true => self.requirements_revision,
            false => self
                .requirements_revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Requirements revision exhausted"))?,
        };
        Ok(Self {
            requirements_revision,
            ..self.clone()
        })
    }

    pub fn with_answer(&self, answer: &AnsweredQuestion) -> anyhow::Result<Self> {
        let mut planning = self.requirements_changed()?;
        planning.record_evidence(format!("answer:{}", answer.id), answer.text.clone());
        Ok(planning)
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
        self.evidence.retain(|id, _| !evicted.contains(id));
    }
}

#[derive(Debug, Clone, Default)]
pub struct InteractionView {
    pub planning: Planning,
    pub questions: Vec<Question>,
}

fn valid_id(id: &str) -> bool {
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
