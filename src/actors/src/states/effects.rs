use crate::actor::Message;
use crate::states::actor_state::ActorState;
use crate::states::provider_task::ProviderTarget;
use analysis::contexts::context::Context;
use commands::command::Command;
use common_models::tui_models::ActorToTuiPacket;
use turn_engine::machine::{Effect, EffectOutcome, ShutdownScope};

pub(super) async fn execute<C: Context + Clone + 'static>(
    actor: &mut ActorState<C>,
    effect: Effect,
) -> EffectOutcome {
    match effect {
        Effect::QueueInput(input) => {
            actor.session_control().queue_input(&input);
            EffectOutcome::Applied
        }
        Effect::BeginTurn(input) => {
            actor.llm.begin_turn();
            let mode = actor.request_mode;
            actor.session_turn().begin(input, mode).await;
            EffectOutcome::Applied
        }
        Effect::AppendHistory(messages) => {
            actor.session_control().append_history(messages);
            EffectOutcome::Applied
        }
        Effect::ClearHistory => {
            match actor.clear_history().await {
                Ok(()) => {
                    actor
                        .reporter
                        .send(ActorToTuiPacket::TokensUpdated(Default::default()));
                    actor.reporter.send(ActorToTuiPacket::CommandResult(
                        Command::Clear,
                        "Started a new session. Previous history remains available through /sessions.".into(),
                    ));
                }
                Err(error) => actor.session_control().persistence.fail(error),
            }
            EffectOutcome::Applied
        }
        Effect::ClearStream => {
            actor.stream.clear();
            EffectOutcome::Applied
        }
        Effect::PreserveCompletedContent => {
            if let Some(message) = actor.stream.take_completed() {
                actor.session_control().append_history(vec![message]);
            }
            EffectOutcome::Applied
        }
        Effect::ChangeState(state) => {
            actor.stream.change_state(state);
            EffectOutcome::Applied
        }
        Effect::Report(packet) => {
            let completed = actor.session_control().persistence.report(packet);
            let merge = match completed {
                Some(turn) => actor.offer_merge(turn).await,
                None => Ok(()),
            };
            if let Err(error) = merge {
                actor.reporter.send(ActorToTuiPacket::SessionError(format!(
                    "Session merge could not continue: {error:#}"
                )));
            }
            EffectOutcome::Applied
        }
        Effect::LaunchProvider {
            run,
            owner,
            previous,
        } => {
            if let Some(previous) = &previous {
                previous.cancel.cancel();
            }
            let usage = actor.stream.usage();
            actor
                .session_control()
                .persistence
                .record(session::Event::Usage(usage));
            actor.provider_context().spawn(
                &actor.llm,
                ProviderTarget {
                    actor: actor.actor_ref.clone(),
                    tag: run.tag,
                },
                &run,
                &owner,
                previous,
            );
            EffectOutcome::Applied
        }
        Effect::LaunchTools { jobs, tag, scope } => {
            let jobs = actor.session_turn().prepare_tools(jobs);
            actor.executor(scope).spawn(jobs, tag);
            EffectOutcome::Applied
        }
        Effect::UpdateContext { tag, result } => {
            actor.reporter.validation(&result);
            actor.interaction_control().record_plan_evidence(&result);
            match actor.services.update_context(&mut actor.context, &result) {
                Ok(()) => EffectOutcome::Applied,
                Err(failure) => EffectOutcome::ContextFailed { tag, failure },
            }
        }
        Effect::Cleanup { turn, scope } => {
            scope.cancel.cancel();
            let actor_ref = actor.actor_ref.clone();
            actor.runtime.scope.tasks.spawn(async move {
                scope.finish().await;
                let _ = actor_ref.send_message(Message::CleanupFinished { turn });
            });
            EffectOutcome::Applied
        }
        Effect::Shutdown(scope) => {
            match scope {
                ShutdownScope::Session => {}
                ShutdownScope::Turn(scope) => scope.finish().await,
            }
            actor.runtime.scope.finish().await;
            EffectOutcome::ShutdownFinished
        }
        Effect::ReplyWorker { request, outcome } => {
            actor
                .worker_replies
                .complete(request, actor.session.worker_result(outcome));
            EffectOutcome::Applied
        }
        Effect::StopActor => {
            actor.actor_ref.stop(None);
            EffectOutcome::Applied
        }
    }
}
