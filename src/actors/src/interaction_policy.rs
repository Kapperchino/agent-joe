use common_models::interaction::WorkMode;
use std::sync::RwLock;
use tools::{
    tool_defs::ToolEffect,
    tool_error::{ToolEffects, ToolFailure, ToolFailureKind},
};

#[derive(Default)]
pub(crate) struct InteractionPolicy(RwLock<Policy>);

#[derive(Default)]
struct Policy {
    mode: WorkMode,
    questions: QuestionGate,
    plan: PlanGate,
}

#[derive(Default)]
enum PlanGate {
    #[default]
    Current,
    Stale,
}

#[derive(Default)]
enum QuestionGate {
    #[default]
    Open,
    Required,
}

impl InteractionPolicy {
    pub fn mode(&self) -> WorkMode {
        self.0.read().unwrap().mode
    }
    pub fn waiting(&self) -> bool {
        matches!(self.0.read().unwrap().questions, QuestionGate::Required)
    }
    pub fn set(&self, mode: WorkMode, required: bool, stale_plan: bool) {
        *self.0.write().unwrap() = Policy {
            mode,
            questions: match required {
                true => QuestionGate::Required,
                false => QuestionGate::Open,
            },
            plan: match stale_plan {
                true => PlanGate::Stale,
                false => PlanGate::Current,
            },
        };
    }

    pub fn authorize(&self, effect: ToolEffect) -> Result<(), ToolFailure> {
        let policy = self.0.read().unwrap();
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
            (_, WorkMode::Implement, _, PlanGate::Stale) => Some(
                "Requirements changed; reconcile the current plan with update_plan before editing, delegating writes or running Cargo",
            ),
            (_, WorkMode::Implement, _, PlanGate::Current) => None,
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
