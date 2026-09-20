pub mod policy;

use common_models::interaction::{
    Answer, AnsweredQuestion, InteractionView, PlanUpdate, Planning, Question, QuestionPurpose,
    Questions, WorkMode,
};

#[derive(Clone, Default)]
pub struct InteractionState {
    planning: Planning,
    questions: Questions,
}

pub struct InteractionUpdate {
    state: InteractionState,
    event: InteractionEvent,
}

pub struct AnsweredInteraction {
    pub update: InteractionUpdate,
    pub answer: AnsweredQuestion,
}

impl InteractionUpdate {
    pub fn commit(
        self,
        record: impl FnOnce(InteractionEvent) -> anyhow::Result<()>,
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
            event: InteractionEvent::Planning(planning.clone()),
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
            event: InteractionEvent::QuestionAsked(question),
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
                    event: InteractionEvent::QuestionsWithdrawn(purpose),
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
                event: InteractionEvent::QuestionAnswered {
                    id: id.into(),
                    answer,
                },
            },
            answer: answered,
        })
    }
    pub fn update_plan(&self, update: PlanUpdate) -> anyhow::Result<InteractionUpdate> {
        let planning = self.planning();
        let plan =
            planning
                .plan
                .update(update, planning.requirements_revision, &planning.evidence)?;
        Ok(self.planned(Planning {
            plan,
            ..planning.clone()
        }))
    }
}

#[derive(Clone)]
pub enum InteractionEvent {
    Planning(Planning),
    QuestionAsked(Question),
    QuestionsWithdrawn(QuestionPurpose),
    QuestionAnswered { id: String, answer: Answer },
}

#[cfg(test)]
mod tests;
