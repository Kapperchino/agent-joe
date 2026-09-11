use super::{PlanReview, bounded_text, valid_id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

impl PlanStep {
    fn supported(&self, graph: &PlanGraph<'_>, evidence: &BTreeMap<String, String>) -> bool {
        let ready = self.dependencies.iter().all(|id| {
            graph
                .by_id
                .get(id.as_str())
                .is_some_and(|step| step.state == StepState::Completed)
        });
        let state_supported = match (&self.state, &self.blocked_reason) {
            (StepState::Blocked, Some(reason)) => bounded_text(reason, 1024),
            (StepState::Pending, None) => true,
            (StepState::InProgress, None) => ready,
            (StepState::Completed, None) => ready && !self.evidence.is_empty(),
            _ => false,
        };
        valid_id(&self.id)
            && bounded_text(&self.description, 512)
            && bounded_text(&self.acceptance, 1024)
            && self.dependencies.len() <= 15
            && self.dependencies.iter().collect::<BTreeSet<_>>().len() == self.dependencies.len()
            && self
                .dependencies
                .iter()
                .all(|id| graph.by_id.contains_key(id.as_str()) && *id != self.id)
            && self.evidence.len() <= 8
            && self.evidence.iter().all(|item| {
                evidence.contains_key(&item.source) && bounded_text(&item.explanation, 512)
            })
            && state_supported
    }

    fn transition(&self, previous: Option<&Self>, review: PlanReview) -> anyhow::Result<Self> {
        let definition_unchanged = previous.is_some_and(|previous| {
            previous.description == self.description
                && previous.acceptance == self.acceptance
                && previous.dependencies == self.dependencies
        });
        match (previous.map(|step| step.state), self.state, review) {
            (_, StepState::Completed, PlanReview::Required) => Err(anyhow::anyhow!(
                "Reopen completed step {} after requirements change",
                self.id
            )),
            (Some(StepState::InProgress), StepState::Completed, _) if definition_unchanged => {
                Ok(self.clone())
            }
            (None | Some(StepState::Pending | StepState::Blocked), StepState::InProgress, _)
            | (_, StepState::Pending | StepState::Blocked, _) => Ok(self.clone()),
            (Some(old), new, _) if old == new && definition_unchanged => Ok(self.clone()),
            _ => Err(anyhow::anyhow!(
                "Step {} has an unsupported transition: start before completing and reopen changed steps",
                self.id
            )),
        }
    }
}

struct PlanGraph<'a> {
    ordered: &'a [PlanStep],
    by_id: BTreeMap<&'a str, &'a PlanStep>,
}

impl<'a> PlanGraph<'a> {
    fn new(steps: &'a [PlanStep], evidence: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        let graph = Self {
            ordered: steps,
            by_id: steps.iter().map(|step| (step.id.as_str(), step)).collect(),
        };
        let shape = !steps.is_empty()
            && steps.len() <= 16
            && graph.by_id.len() == steps.len()
            && steps
                .iter()
                .filter(|step| step.state == StepState::InProgress)
                .count()
                <= 1
            && steps.iter().all(|step| step.supported(&graph, evidence));
        match shape {
            false => Err(anyhow::anyhow!(
                "Invalid plan: use 1–16 unique bounded steps, valid dependencies, recorded evidence and supported states"
            )),
            true if graph.reachable().len() != steps.len() => {
                Err(anyhow::anyhow!("Plan dependencies contain a cycle"))
            }
            true => Ok(graph),
        }
    }

    fn reachable(&self) -> BTreeSet<&str> {
        (0..self.ordered.len()).fold(BTreeSet::new(), |reachable, _| {
            self.ordered
                .iter()
                .filter(|step| {
                    step.dependencies
                        .iter()
                        .all(|id| reachable.contains(id.as_str()))
                })
                .map(|step| step.id.as_str())
                .collect()
        })
    }

    fn transition(&self, previous: &Plan, review: PlanReview) -> anyhow::Result<Vec<PlanStep>> {
        match previous.steps.iter().all(|step| {
            step.state != StepState::Completed || self.by_id.contains_key(step.id.as_str())
        }) {
            true => self
                .ordered
                .iter()
                .map(|step| {
                    let previous = previous
                        .steps
                        .iter()
                        .find(|previous| previous.id == step.id);
                    step.transition(previous, review)
                })
                .collect(),
            false => Err(anyhow::anyhow!(
                "Reopen a completed step before removing it"
            )),
        }
    }
}

impl Plan {
    pub fn update(
        &self,
        update: PlanUpdate,
        requirements_revision: u64,
        evidence: &BTreeMap<String, String>,
    ) -> anyhow::Result<Self> {
        let graph = match update.revision == self.revision
            && update.requirements_revision == requirements_revision
        {
            true => PlanGraph::new(&update.steps, evidence),
            false => Err(anyhow::anyhow!(
                "Stale plan: use the current plan and requirements revisions"
            )),
        }?;
        let review = match self.requirements_revision == requirements_revision {
            true => PlanReview::Current,
            false => PlanReview::Required,
        };
        Ok(Self {
            revision: self
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Plan revision exhausted"))?,
            requirements_revision,
            steps: graph.transition(self, review)?,
        })
    }
}
