use super::{Event, persistence::Persistence};
use crate::states::runtime::ExecutionRole;
use common_models::interaction::{PlanUpdate, Question, QuestionPurpose};
use interaction::{InteractionEvent, InteractionState, InteractionUpdate};
use utils::execution::ExecutionScope;

impl From<InteractionEvent> for Event {
    fn from(event: InteractionEvent) -> Self {
        match event {
            InteractionEvent::Planning(planning) => Self::Planning(planning),
            InteractionEvent::QuestionAsked(question) => Self::QuestionAsked(question),
            InteractionEvent::QuestionsWithdrawn(purpose) => Self::QuestionsWithdrawn(purpose),
            InteractionEvent::QuestionAnswered { id, answer } => {
                Self::QuestionAnswered { id, answer }
            }
        }
    }
}

pub struct Interaction<'a> {
    state: &'a InteractionState,
}

impl<'a> Interaction<'a> {
    pub fn new(
        state: &'a InteractionState,
        role: &ExecutionRole,
        persistence: &Persistence,
        scope: &ExecutionScope,
    ) -> anyhow::Result<Self> {
        match (persistence, scope.cancel.is_cancelled(), role) {
            (Persistence::Ready, false, ExecutionRole::Root) => Ok(Self { state }),
            (Persistence::Failed(failure), _, _) => Err(anyhow::anyhow!(failure.to_string())),
            (_, true, _) => Err(anyhow::anyhow!("Interaction cancelled")),
            (_, _, ExecutionRole::Worker { .. } | ExecutionRole::Helper) => Err(anyhow::anyhow!(
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
