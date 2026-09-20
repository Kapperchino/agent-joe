use crate::{InteractionState, InteractionUpdate};
use common_models::interaction::{PlanUpdate, Question, QuestionPurpose};
use utils::execution::ExecutionScope;

pub struct Interaction<'a> {
    state: &'a InteractionState,
}

#[derive(Clone, Copy)]
pub enum InteractionReadiness {
    Idle,
    Active,
    Stopping,
}

#[derive(Clone, Copy)]
pub enum InteractionRole {
    Root,
    Delegated,
}

impl<'a> Interaction<'a> {
    pub fn new(
        state: &'a InteractionState,
        role: InteractionRole,
        scope: &ExecutionScope,
    ) -> anyhow::Result<Self> {
        match (scope.cancel.is_cancelled(), role) {
            (false, InteractionRole::Root) => Ok(Self { state }),
            (true, _) => Err(anyhow::anyhow!("Interaction cancelled")),
            (_, InteractionRole::Delegated) => Err(anyhow::anyhow!(
                "Only the root can change the plan or ask the user; report questions to the parent"
            )),
        }
    }

    pub fn ask(self, question: Question) -> anyhow::Result<InteractionUpdate> {
        match question.purpose {
            QuestionPurpose::Clarification => self.state.asked(question),
            QuestionPurpose::Merge => Err(anyhow::anyhow!(
                "Only the runtime can request merge approval"
            )),
        }
    }

    pub fn update_plan(self, update: PlanUpdate) -> anyhow::Result<InteractionUpdate> {
        self.state.update_plan(update)
    }
}
