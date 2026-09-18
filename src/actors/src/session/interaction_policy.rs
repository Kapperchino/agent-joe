use common_models::interaction::{PlanReview, Planning, QuestionGate, Questions, WorkMode};
use std::sync::RwLock;
use tools::{
    tool_defs::ToolEffect,
    tool_error::{ToolEffects, ToolFailure, ToolFailureKind},
};

#[derive(Default)]
pub(crate) struct InteractionPolicy {
    policy: RwLock<Policy>,
}

#[derive(Default)]
struct Policy {
    mode: WorkMode,
    questions: QuestionGate,
    plan: PlanReview,
}

impl InteractionPolicy {
    pub fn mode(&self) -> WorkMode {
        self.policy.read().unwrap().mode
    }
    pub fn waiting(&self) -> bool {
        self.policy.read().unwrap().questions == QuestionGate::Required
    }
    pub fn set(&self, planning: &Planning, questions: &Questions) {
        *self.policy.write().unwrap() = Policy {
            mode: planning.mode,
            questions: questions.gate(),
            plan: planning.review(),
        };
    }

    pub fn authorize(&self, effect: ToolEffect) -> Result<(), ToolFailure> {
        let policy = self.policy.read().unwrap();
        let denied = match (&policy.questions, policy.mode, effect, &policy.plan) {
            (QuestionGate::Required, _, _, _) => {
                Some("A required question is pending; answer it before continuing tools")
            }
            (_, _, ToolEffect::Read | ToolEffect::DelegateRead | ToolEffect::Interaction, _) => {
                None
            }
            (_, WorkMode::Plan, _, _) => Some(
                "Plan mode permits read-only investigation; use /implement to enable changes and Cargo",
            ),
            (_, WorkMode::Implement, _, PlanReview::Required) => Some(
                "Requirements changed; reconcile the current plan with update_plan before editing, delegating writes or running Cargo",
            ),
            (_, WorkMode::Implement, _, PlanReview::Current) => None,
        };
        match denied {
            Some(message) => Err(ToolFailure::new(
                ToolFailureKind::InvalidInput,
                ToolEffects::NotStarted,
                message,
            )),
            None => Ok(()),
        }
    }
}
