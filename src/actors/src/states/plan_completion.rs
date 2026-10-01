use super::actor_state::ActorState;
use super::runtime::ExecutionRole;
use analysis::contexts::context::Context;
use clients::llm::Message;
use clients::response::RequestMode;
use commands::command::{Command, QuestionAnswer};
use common_models::interaction::{QuestionPurpose, WorkMode};
use common_models::runtime_ids::TurnId;
use common_models::tui_models::ActorToTuiPacket;
use session::activation::SessionActivation;
use session::plan::{PlanContinuation, PlanHandoff};
use turn_engine::machine::SessionEvent;
use turn_engine::turn::FollowUp;

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(super) fn offer_plan_continuation(&mut self, turn: TurnId) -> anyhow::Result<()> {
        match (
            &self.runtime.role,
            self.session
                .conversation
                .request_mode(turn, self.request_mode),
            self.session.interaction.planning().mode,
            self.turn.is_idle(),
            self.session.interaction.questions().pending(),
        ) {
            (ExecutionRole::Root, RequestMode::Continue, WorkMode::Plan, true, []) => {
                PlanHandoff::new(&self.session)?;
                self.interaction_control()
                    .ask_question(PlanHandoff::question(turn)?)
            }
            _ => Ok(()),
        }
    }

    pub(super) fn is_plan_answer(&self, input: &QuestionAnswer) -> bool {
        self.session
            .interaction
            .questions()
            .pending()
            .iter()
            .any(|question| {
                question.id == input.id && question.purpose == QuestionPurpose::PlanContinuation
            })
    }

    pub(super) async fn answer_plan(&mut self, input: &QuestionAnswer) -> anyhow::Result<String> {
        match (self.turn.is_idle(), &self.runtime.role, self.request_mode) {
            (true, ExecutionRole::Root, RequestMode::Continue) => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Finish the active turn before choosing how to implement the plan"
            )),
        }?;
        self.session
            .interaction
            .answered(&input.id, input.answer.clone())?;
        let continuation = PlanContinuation::try_from(&input.answer)?;
        match continuation {
            PlanContinuation::KeepPlanning => {
                self.session_turn().answer(input).await?;
                Ok("Kept plan mode. Send a follow-up to refine the plan.".into())
            }
            PlanContinuation::Implement | PlanContinuation::NewAgent => {
                let handoff = PlanHandoff::new(&self.session)?;
                let activation = match continuation {
                    PlanContinuation::NewAgent => Some(
                        SessionActivation::from_plan(
                            &self.context,
                            &self.runtime.session_runtime(),
                            &self.llm,
                            &handoff,
                        )
                        .await?,
                    ),
                    _ => None,
                };
                self.session_turn().answer(input).await?;
                let message = match activation {
                    Some(activation) => {
                        self.activate_session(activation).await?;
                        self.reporter.send(ActorToTuiPacket::SessionChanged);
                        self.interaction_control().refresh_interaction();
                        self.reporter
                            .send(ActorToTuiPacket::TokensUpdated(self.stream.usage()));
                        "Started a new agent with the approved plan. The planning session remains available through /sessions."
                    }
                    None => {
                        self.session_turn().command(&Command::Implement)?;
                        self.session_control().append_history(vec![Message::new(
                            "Implement the approved plan in this session. Continue its tracked implementation steps and run the planned validation.".into(),
                        )]);
                        "Implementing the approved plan in this session."
                    }
                };
                self.dispatch(SessionEvent::Start(FollowUp::new(None)))
                    .await;
                Ok(message.into())
            }
        }
    }
}
