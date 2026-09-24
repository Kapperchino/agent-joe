pub mod execution;
use common_models::{
    interaction::{Answer, Choice, Question, QuestionPurpose},
    runtime_ids::{OperationId, TurnId},
};
use utils::git::worktrees::session::MergeConflict;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum MergeApproval {
    #[default]
    None,
    Awaiting {
        question: String,
        commit: String,
    },
    Approved {
        commit: String,
    },
    Resolving {
        conflict: MergeConflict,
        #[serde(skip)]
        activity: ResolutionActivity,
    },
}

#[derive(Clone, Default)]
pub enum ResolutionActivity {
    #[default]
    Paused,
    Running {
        turn: TurnId,
    },
}

pub enum MergeEvent {
    TaskStarted {
        turn: TurnId,
    },
    Proposed {
        commit: String,
    },
    Approved {
        commit: String,
    },
    Conflicted {
        conflict: MergeConflict,
        turn: TurnId,
    },
    Paused,
    Finished,
}

pub enum MergeProposal {
    Empty,
    AwaitingApproval { commit: String },
    Approved { commit: String },
}

impl MergeProposal {
    pub fn new(approval: &MergeApproval, turn: TurnId, commit: Option<String>) -> Self {
        match commit {
            Some(commit) if approval.resolution(turn).is_some() => Self::Approved { commit },
            Some(commit) => Self::AwaitingApproval { commit },
            None => Self::Empty,
        }
    }

    pub fn event(&self) -> MergeEvent {
        match self {
            Self::Empty => MergeEvent::Finished,
            Self::AwaitingApproval { commit } => MergeEvent::Proposed {
                commit: commit.clone(),
            },
            Self::Approved { commit } => MergeEvent::Approved {
                commit: commit.clone(),
            },
        }
    }
}

pub enum MergeDecision {
    Merge { commit: String },
    Keep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeReadiness {
    Ready,
    TaskActive,
    StorageFailed,
}

impl MergeDecision {
    pub fn new(
        approval: &MergeApproval,
        id: &str,
        answer: &Answer,
        readiness: MergeReadiness,
    ) -> anyhow::Result<Self> {
        approval
            .question()
            .filter(|question| question.id == id)
            .ok_or_else(|| anyhow::anyhow!("No merge is awaiting approval"))?
            .answer(answer)?;
        match (readiness, approval, answer) {
            (
                MergeReadiness::Ready,
                MergeApproval::Awaiting { commit, .. },
                Answer::Choice { choice_id },
            ) if choice_id == "merge" => Ok(Self::Merge {
                commit: commit.clone(),
            }),
            (MergeReadiness::Ready, _, _) => Ok(Self::Keep),
            _ => Err(anyhow::anyhow!(
                "Finish the active task before answering the merge question"
            )),
        }
    }
}

impl MergeApproval {
    pub fn resolution(&self, turn: TurnId) -> Option<&MergeConflict> {
        match self {
            Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn: approved },
            } if *approved == turn => Some(conflict),
            _ => None,
        }
    }

    pub fn transition(&self, event: MergeEvent) -> Option<Self> {
        match (self, event) {
            (Self::Awaiting { .. } | Self::Approved { .. }, MergeEvent::TaskStarted { .. }) => {
                Some(Self::None)
            }
            (
                Self::Resolving {
                    activity: ResolutionActivity::Running { turn: approved },
                    ..
                },
                MergeEvent::TaskStarted { turn },
            ) if *approved == turn => None,
            (
                Self::Resolving {
                    conflict,
                    activity: ResolutionActivity::Running { .. },
                },
                MergeEvent::TaskStarted { .. } | MergeEvent::Paused,
            ) => Some(Self::Resolving {
                conflict: conflict.clone(),
                activity: ResolutionActivity::Paused,
            }),
            (_, MergeEvent::TaskStarted { .. } | MergeEvent::Paused) => None,
            (_, MergeEvent::Proposed { commit }) => Some(Self::Awaiting {
                question: format!("merge-{}", OperationId::new()),
                commit,
            }),
            (_, MergeEvent::Approved { commit }) => Some(Self::Approved { commit }),
            (_, MergeEvent::Conflicted { conflict, turn }) => Some(Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn },
            }),
            (_, MergeEvent::Finished) => Some(Self::None),
        }
    }

    pub fn recovery(&self, pending: &[Question]) -> Option<MergeEvent> {
        let commit = match self {
            Self::Awaiting { question, commit }
                if !pending
                    .iter()
                    .filter(|pending| pending.purpose == QuestionPurpose::Merge)
                    .any(|pending| pending.id == *question) =>
            {
                Some(commit)
            }
            Self::Approved { commit } => Some(commit),
            _ => None,
        };
        commit.map(|commit| MergeEvent::Proposed {
            commit: commit.clone(),
        })
    }

    pub fn question(&self) -> Option<Question> {
        match self {
            Self::None | Self::Approved { .. } | Self::Resolving { .. } => None,
            Self::Awaiting { question, commit } => Some(Question {
                purpose: QuestionPurpose::Merge,
                id: question.clone(),
                prompt: format!(
                    "Task completed successfully. Merge session commit {commit} into main, resolving any conflicts, then delete the entire session workspace, including ignored files and build caches?"
                ),
                required: false,
                choices: vec![
                    Choice {
                        id: "merge".into(),
                        label: "Merge into main".into(),
                    },
                    Choice {
                        id: "keep".into(),
                        label: "Keep changes in this session".into(),
                    },
                ],
                allow_free_text: false,
            }),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
