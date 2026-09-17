use common_models::interaction::{Plan, Planning, Question, WorkMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanningState {
    pub mode: WorkMode,
    pub plan: Plan,
    pub requirements_revision: u64,
}

impl From<&Planning> for PlanningState {
    fn from(planning: &Planning) -> Self {
        Self {
            mode: planning.mode,
            plan: planning.plan.clone(),
            requirements_revision: planning.requirements_revision,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub planning: PlanningState,
    pub evidence: BTreeMap<String, String>,
    pub questions: Vec<Question>,
    pub workers: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning: Option<PlanningState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub evidence: BTreeMap<String, Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<Vec<Question>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeUpdate {
    Snapshot(RuntimeSnapshot),
    Changes(RuntimeChanges),
}

impl RuntimeSnapshot {
    pub fn update(&self, previous: Option<&Self>) -> Option<RuntimeUpdate> {
        match previous {
            None => Some(RuntimeUpdate::Snapshot(self.clone())),
            Some(previous) => {
                let changes = RuntimeChanges {
                    planning: (self.planning != previous.planning).then(|| self.planning.clone()),
                    evidence: self
                        .evidence
                        .iter()
                        .filter(|(id, detail)| previous.evidence.get(*id) != Some(*detail))
                        .map(|(id, detail)| (id.clone(), Some(detail.clone())))
                        .chain(
                            previous
                                .evidence
                                .keys()
                                .filter(|id| !self.evidence.contains_key(*id))
                                .map(|id| (id.clone(), None)),
                        )
                        .collect(),
                    questions: (self.questions != previous.questions)
                        .then(|| self.questions.clone()),
                    workers: (self.workers != previous.workers).then(|| self.workers.clone()),
                };
                (changes != RuntimeChanges::default()).then_some(RuntimeUpdate::Changes(changes))
            }
        }
    }

    pub fn apply(self, update: &RuntimeUpdate) -> Self {
        match update {
            RuntimeUpdate::Snapshot(snapshot) => snapshot.clone(),
            RuntimeUpdate::Changes(changes) => Self {
                planning: changes.planning.clone().unwrap_or(self.planning),
                evidence: self
                    .evidence
                    .into_iter()
                    .filter(|(id, _)| !changes.evidence.contains_key(id))
                    .chain(changes.evidence.iter().filter_map(|(id, detail)| {
                        detail.as_ref().map(|detail| (id.clone(), detail.clone()))
                    }))
                    .collect(),
                questions: changes.questions.clone().unwrap_or(self.questions),
                workers: changes.workers.clone().unwrap_or(self.workers),
            },
        }
    }
}

impl RuntimeUpdate {
    pub fn text(&self) -> anyhow::Result<String> {
        Ok(format!(
            "Runtime state update:\n{}",
            serde_json::to_string(self)?
        ))
    }
}
