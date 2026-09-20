use super::{Event, persistence::Persistence};
use crate::states::runtime::ExecutionRole;
use common_models::interaction::{
    Answer, AnsweredQuestion, InteractionView, PlanUpdate, Planning, Question, QuestionPurpose,
    Questions, WorkMode,
};
use utils::execution::ExecutionScope;

#[derive(Clone, Default)]
pub struct InteractionState {
    planning: Planning,
    questions: Questions,
}

pub struct InteractionUpdate {
    state: InteractionState,
    event: Event,
}

pub struct AnsweredInteraction {
    pub update: InteractionUpdate,
    pub answer: AnsweredQuestion,
}

impl InteractionUpdate {
    pub fn commit(
        self,
        record: impl FnOnce(Event) -> anyhow::Result<()>,
    ) -> anyhow::Result<InteractionState> {
        record(self.event)?;
        Ok(self.state)
    }
}

impl InteractionState {
    pub fn new(mode: WorkMode) -> Self {
        Self::restored(
            Planning {
                mode,
                ..Default::default()
            },
            Questions::default(),
        )
    }

    pub fn restored(planning: Planning, questions: Questions) -> Self {
        Self {
            planning,
            questions,
        }
    }

    pub fn planning(&self) -> &Planning {
        &self.planning
    }

    pub fn questions(&self) -> &Questions {
        &self.questions
    }

    pub fn view(&self) -> InteractionView {
        InteractionView {
            planning: self.planning.clone(),
            questions: self.questions.pending().to_vec(),
        }
    }

    pub fn content(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&serde_json::json!({
            "planning": self.planning,
            "pending_questions": self.questions.pending(),
        }))?)
    }

    pub fn planned(&self, planning: Planning) -> InteractionUpdate {
        InteractionUpdate {
            event: Event::Planning(planning.clone()),
            state: Self {
                planning,
                questions: self.questions.clone(),
            },
        }
    }

    pub fn requirements_changed(&self) -> anyhow::Result<Option<InteractionUpdate>> {
        match self.planning.plan.steps.as_slice() {
            [] => Ok(None),
            _ => self
                .planning
                .requirements_changed()
                .map(|planning| Some(self.planned(planning))),
        }
    }

    pub fn asked(&self, question: Question) -> anyhow::Result<InteractionUpdate> {
        let mut state = self.clone();
        state.questions.ask(question.clone())?;
        Ok(InteractionUpdate {
            state,
            event: Event::QuestionAsked(question),
        })
    }

    pub fn withdrawn(&self, purpose: QuestionPurpose) -> Option<InteractionUpdate> {
        self.questions
            .pending()
            .iter()
            .any(|question| question.purpose == purpose)
            .then(|| {
                let mut state = self.clone();
                state.questions.withdraw(purpose);
                InteractionUpdate {
                    state,
                    event: Event::QuestionsWithdrawn(purpose),
                }
            })
    }

    pub fn answered(&self, id: &str, answer: Answer) -> anyhow::Result<AnsweredInteraction> {
        let mut questions = self.questions.clone();
        let answered = questions.answer(id, &answer)?;
        let planning = self.planning.with_answer(&answered)?;
        Ok(AnsweredInteraction {
            update: InteractionUpdate {
                state: Self {
                    planning,
                    questions,
                },
                event: Event::QuestionAnswered {
                    id: id.into(),
                    answer,
                },
            },
            answer: answered,
        })
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
        let planning = self.state.planning();
        let plan =
            planning
                .plan
                .update(update, planning.requirements_revision, &planning.evidence)?;
        Ok(self.state.planned(Planning {
            plan,
            ..planning.clone()
        }))
    }
}
