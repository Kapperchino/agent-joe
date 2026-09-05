use crate::{
    actor::{Dependency, Message},
    actor_state::ActorState,
    provider_task::{self, ProviderEvent},
    turn::{HistoryDisposition, Tag},
    turn_machine::{
        Effect, EffectOutcome, Event, ProviderUpdate, SessionEvent, ShutdownScope, WorkerOutcome,
    },
};
use analysis::contexts::context::Context;
use clients::{
    failure::{Failure, FailureKind},
    llm,
};
use commands::command::Command;
use common_models::tui_models::ActorToTuiPacket;
use std::{collections::VecDeque, panic::AssertUnwindSafe};
use tools::tool_defs::ToolResult;

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) async fn dispatch(&mut self, event: impl Into<Event>) {
        let mut effects = VecDeque::from(self.turn.transition(event));
        while let Some(effect) = effects.pop_front() {
            let outcome = self.execute(effect).await;
            let next = self.turn.feedback(outcome);
            for effect in next.into_iter().rev() {
                effects.push_front(effect);
            }
        }
    }

    async fn execute(&mut self, effect: Effect) -> EffectOutcome {
        match effect {
            Effect::AppendHistory(messages) => {
                self.history.extend(messages);
                EffectOutcome::Applied
            }
            Effect::ClearHistory => {
                self.clear_history().await;
                self.reporter.send(ActorToTuiPacket::CommandResult(
                    Command::Clear,
                    "History cleared".into(),
                ));
                EffectOutcome::Applied
            }
            Effect::ClearStream => {
                self.stream_processor.clear();
                EffectOutcome::Applied
            }
            Effect::PreserveCompletedContent => {
                self.preserve_completed_content();
                EffectOutcome::Applied
            }
            Effect::ChangeState(state) => {
                self.change_state(state);
                EffectOutcome::Applied
            }
            Effect::Report(packet) => {
                self.reporter.send(packet);
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
                provider_task::spawn(
                    self.actor_ref.clone(),
                    self.llm.clone(),
                    self.build_request(),
                    &run,
                    &owner,
                    previous,
                    self.dependency.runtime.request_timeout,
                );
                EffectOutcome::Applied
            }
            Effect::LaunchTools { jobs, tag, scope } => {
                self.executor(scope).spawn(jobs, tag);
                EffectOutcome::Applied
            }
            Effect::UpdateContext { tag, result } => {
                match update_tool_context(&self.dependency, &mut self.cur_context, &result) {
                    Ok(()) => EffectOutcome::Applied,
                    Err(failure) => EffectOutcome::ContextFailed { tag, failure },
                }
            }
            Effect::Cleanup { turn, scope } => {
                scope.cancel.cancel();
                let actor = self.actor_ref.clone();
                self.dependency.runtime.scope.tasks.spawn(async move {
                    scope.finish().await;
                    let _ = actor.send_message(Message::CleanupFinished { turn });
                });
                EffectOutcome::Applied
            }
            Effect::Shutdown(scope) => {
                match scope {
                    ShutdownScope::Session => {}
                    ShutdownScope::Turn(scope) => scope.finish().await,
                }
                self.dependency.runtime.scope.finish().await;
                EffectOutcome::ShutdownFinished
            }
            Effect::ReplyWorker { reply, outcome } => {
                let result = match outcome {
                    WorkerOutcome::Completed => Ok(self
                        .history
                        .last()
                        .map(llm::Message::text)
                        .unwrap_or_default()),
                    WorkerOutcome::Failed(failure) => Err(failure),
                };
                let _ = reply.send(result);
                EffectOutcome::Applied
            }
            Effect::StopActor => {
                self.actor_ref.stop(None);
                EffectOutcome::Applied
            }
        }
    }

    pub(crate) async fn provider_event(&mut self, tag: Tag, event: ProviderEvent) {
        if let Some(response) = self.turn.provider_response(tag) {
            let update = match event {
                ProviderEvent::Item(item) => {
                    match self.stream_processor.process_stream_event(item).await {
                        Ok(step) => ProviderUpdate::Progress(step),
                        Err(error) => ProviderUpdate::Finished(Err(provider_input_error(error))),
                    }
                }
                ProviderEvent::Finished(Err(failure)) => ProviderUpdate::Finished(Err(failure)),
                ProviderEvent::Finished(Ok(())) => {
                    ProviderUpdate::Finished(response.finish(tag.turn, &mut self.stream_processor))
                }
            };
            self.dispatch(SessionEvent::Provider { tag, update }).await;
        }
    }

    fn preserve_completed_content(&mut self) {
        if let Some(batch) = self.stream_processor.batches.last() {
            let content = batch.completed_content();
            if !content.is_empty() {
                self.history.push(llm::Message {
                    role: llm::Role::Assistant,
                    content,
                });
            }
        }
        self.stream_processor.clear();
    }

    #[cfg(test)]
    pub(crate) fn visible_history(&self) -> Vec<llm::Message> {
        let mut history = self.history.clone();
        if let Some(batch) = self.turn.batch() {
            history.extend(batch.messages());
        }
        history
    }

    pub(crate) async fn command(&mut self, command: Command) {
        match command {
            Command::Clear => {
                self.dispatch(SessionEvent::Interrupt(HistoryDisposition::Clear))
                    .await
            }
            Command::PrintContext => {
                let context = self.cur_context.clone();
                let reporter = self.reporter.clone();
                let scope = self.dependency.runtime.scope.clone();
                scope.tasks.clone().spawn(async move {
                    tokio::select! {
                        _ = scope.cancel.cancelled() => {},
                        text = context.get_ctx() => reporter.send(ActorToTuiPacket::CommandResult(Command::PrintContext, text)),
                    }
                });
            }
            Command::Logout => {
                let result = clients::config::Config::delete()
                    .await
                    .map(|_| "Logged out. Removed config".to_owned())
                    .unwrap_or_else(|err| format!("Deletion failed: {err}"));
                self.reporter
                    .send(ActorToTuiPacket::CommandResult(Command::Logout, result));
            }
            Command::ChangeModel(name, effort) => {
                if let Err(error) = self
                    .llm
                    .change_model_and_effort(name.clone(), effort.clone())
                    .await
                {
                    self.reporter.send(ActorToTuiPacket::CommandResult(
                        Command::ChangeModel(name, effort),
                        error.to_string(),
                    ));
                }
            }
        }
    }
}

fn provider_input_error(error: anyhow::Error) -> Failure {
    error
        .downcast_ref::<Failure>()
        .cloned()
        .unwrap_or_else(|| Failure::new(FailureKind::InvalidInput, error.to_string()))
}

fn update_tool_context<C: Context>(
    dependency: &Dependency<C>,
    context: &mut C,
    result: &ToolResult,
) -> Result<(), Failure> {
    match (
        &result.outcome,
        dependency.tool(result.invocation.name.as_ref()),
    ) {
        (Ok(content), Some(tool)) => {
            let input = serde_json::Value::Object(result.invocation.input.clone());
            match std::panic::catch_unwind(AssertUnwindSafe(|| {
                tool.add_context(&input, context, content)
            })) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(Failure::new(
                    FailureKind::Tool,
                    format!("Tool completed but context update failed: {error}"),
                )),
                Err(_) => Err(Failure::new(
                    FailureKind::Tool,
                    "Tool completed but context hook panicked",
                )),
            }
        }
        _ => Ok(()),
    }
}
