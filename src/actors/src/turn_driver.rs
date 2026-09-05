use crate::{
    actor::{Dependency, Message},
    actor_state::ActorState,
    provider_task::{self, ProviderEvent},
    scheduler::ToolEvent,
    stream_processor::StreamNextStep,
    turn::{
        AcceptedResponse, Cleanup, CleanupWork, FollowUp, HistoryDisposition, ProviderRun,
        ResponseState, Tag, ToolBatch, Turn, TurnOutcome, TurnState, tool_failure,
    },
    worker::WorkerFailure,
};
use analysis::contexts::context::Context;
use clients::{
    failure::{Failure, FailureKind},
    llm,
};
use commands::command::Command;
use common_models::{
    runtime_ids::TurnId,
    tui_models::{ActorToTuiPacket, Lifecycle, State},
};
use ractor::RpcReplyPort;
use std::panic::AssertUnwindSafe;
use tools::tool_defs::ToolResult;
use utils::execution::ExecutionScope;

pub enum SessionRole {
    Interactive,
    Worker(RpcReplyPort<Result<String, WorkerFailure>>),
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) fn start_work(&mut self, prompt: Option<String>) {
        let follow_up = FollowUp::new(prompt);
        if matches!(self.turn, TurnState::Idle) {
            self.begin(follow_up);
        } else {
            let id = follow_up.id;
            self.queue.push_back(follow_up);
            self.reporter.send(ActorToTuiPacket::Queued {
                turn_id: id,
                position: self.queue.len(),
            });
        }
    }

    pub(crate) fn start_worker(&mut self, reply: RpcReplyPort<Result<String, WorkerFailure>>) {
        match (&self.turn, &self.role) {
            (TurnState::Idle, SessionRole::Interactive) => {
                self.role = SessionRole::Worker(reply);
                self.begin(FollowUp::new(None));
            }
            _ => {
                let _ = reply.send(Err(WorkerFailure::AlreadyRunning));
            }
        }
    }

    fn begin(&mut self, follow_up: FollowUp) {
        if let Some(prompt) = follow_up.prompt {
            self.history.push(llm::Message::new(prompt));
        }
        self.launch_provider(
            Turn::new(follow_up.id, self.dependency.runtime.scope.child()),
            None,
        );
    }

    fn launch_provider(&mut self, turn: Turn<ProviderRun>, previous: Option<ExecutionScope>) {
        self.stream_processor.clear();
        self.change_state(State::StreamStart);
        self.reporter.turn(turn.id, Lifecycle::Running, None);
        self.reporter
            .operation(turn.phase.tag, Lifecycle::Running, "Provider request");
        provider_task::spawn(
            self.actor_ref.clone(),
            self.llm.clone(),
            self.build_request(),
            &turn.phase,
            &turn.scope,
            previous,
            self.dependency.runtime.request_timeout,
        );
        self.turn = TurnState::Provider(turn);
    }

    pub(crate) async fn provider_event(&mut self, tag: Tag, event: ProviderEvent) {
        match std::mem::take(&mut self.turn) {
            TurnState::Provider(turn) if turn.phase.tag == tag => {
                self.apply_provider_event(turn, event).await;
            }
            obsolete => self.turn = obsolete,
        }
    }

    async fn apply_provider_event(&mut self, mut turn: Turn<ProviderRun>, event: ProviderEvent) {
        match event {
            ProviderEvent::Item(event) => {
                match self.stream_processor.process_stream_event(event).await {
                    Ok(step) => {
                        match step {
                            StreamNextStep::Done => turn.phase.response = ResponseState::Complete,
                            StreamNextStep::ToolUse => turn.phase.response = ResponseState::ToolUse,
                            StreamNextStep::Accum | StreamNextStep::Noop => {}
                        }
                        self.turn = TurnState::Provider(turn);
                    }
                    Err(error) => self.provider_failed(turn, provider_input_error(error)),
                }
            }
            ProviderEvent::Finished(Err(failure)) => self.provider_failed(turn, failure),
            ProviderEvent::Finished(Ok(())) => self.provider_finished(turn),
        }
    }

    fn provider_finished(&mut self, turn: Turn<ProviderRun>) {
        match turn.phase.finish(&mut self.stream_processor) {
            Ok(response) => self.accept_response(turn, response),
            Err(failure) => self.provider_failed(turn, failure),
        }
    }

    fn accept_response(&mut self, turn: Turn<ProviderRun>, response: AcceptedResponse) {
        self.reporter.operation(
            turn.phase.tag,
            Lifecycle::Completed,
            "Provider response received",
        );
        match response {
            AcceptedResponse::Tools(batch) => {
                self.reporter
                    .turn(turn.id, Lifecycle::WaitingForTools, None);
                self.reporter
                    .operation(batch.tag, Lifecycle::WaitingForTools, "Tool batch");
                self.change_state(State::ToolStart);
                self.executor(turn.scope.clone())
                    .spawn(batch.jobs(), batch.tag);
                self.turn = TurnState::Tools(turn.map(|_| batch));
            }
            AcceptedResponse::Complete(message) => {
                self.history.push(message);
                self.stop(turn, TurnOutcome::Completed, HistoryDisposition::Retain);
            }
        }
    }

    fn provider_failed(&mut self, turn: Turn<ProviderRun>, failure: Failure) {
        self.reporter
            .operation(turn.phase.tag, Lifecycle::Failed, failure.to_string());
        if failure.retryable() && turn.phase.attempt < 2 {
            let previous = turn.phase.scope.clone();
            previous.cancel.cancel();
            let attempt = turn.phase.attempt + 1;
            let run = ProviderRun::new(turn.id, turn.scope.child(), attempt);
            self.reporter.turn(
                turn.id,
                Lifecycle::Running,
                Some(format!(
                    "{failure}. Retrying the interrupted provider request."
                )),
            );
            self.launch_provider(turn.map(|_| run), Some(previous));
        } else {
            self.stop(
                turn,
                TurnOutcome::Failed(failure),
                HistoryDisposition::Retain,
            );
        }
    }

    pub(crate) fn tool_event(&mut self, tag: Tag, event: ToolEvent) {
        match event {
            ToolEvent::Started {
                operation,
                effect,
                revision,
                display,
            } => {
                if let Some((_, batch)) = self.turn.tools_mut(tag)
                    && batch.start(operation).is_some()
                {
                    self.reporter.send(ActorToTuiPacket::ToolUse(vec![display]));
                    let detail = match revision {
                        Some(revision) => {
                            format!("{effect:?} tool at workspace revision {revision}")
                        }
                        None => format!("{effect:?} worker"),
                    };
                    self.reporter
                        .operation(Tag { operation, ..tag }, Lifecycle::Running, detail);
                }
            }
            ToolEvent::Completed { operation, result } => {
                if let Some((failures, batch)) = self.turn.tools_mut(tag)
                    && let Some(result) = batch.complete(operation, result)
                {
                    let continuation =
                        update_tool_context(&self.dependency, &mut self.cur_context, &result)
                            .and_then(|()| {
                                failures
                                    .record(&result, self.dependency.runtime.workspace.revision())
                            });
                    if continuation.is_err() {
                        batch.continuation = continuation;
                    }
                    let (status, detail) = match &result.outcome {
                        Ok(_) => (Lifecycle::Completed, result.invocation.display.clone()),
                        Err(failure) => (Lifecycle::Failed, failure.to_string()),
                    };
                    self.reporter
                        .operation(Tag { operation, ..tag }, status, detail);
                }
            }
            ToolEvent::Finished(result) => self.tools_finished(tag, result),
        }
    }

    fn tools_finished(&mut self, tag: Tag, result: Result<(), tools::tool_error::ToolFailure>) {
        match std::mem::take(&mut self.turn) {
            TurnState::Tools(turn) if turn.phase.tag == tag => self.finish_tools(turn, result),
            obsolete => self.turn = obsolete,
        }
    }

    fn finish_tools(
        &mut self,
        turn: Turn<ToolBatch>,
        result: Result<(), tools::tool_error::ToolFailure>,
    ) {
        self.change_state(State::ToolStop);
        match result
            .map_err(|failure| tool_failure(&failure))
            .and(turn.phase.continuation.clone())
        {
            Err(failure) => {
                self.reporter
                    .operation(turn.phase.tag, Lifecycle::Failed, failure.to_string());
                self.stop(
                    turn,
                    TurnOutcome::Failed(failure),
                    HistoryDisposition::Retain,
                );
            }
            Ok(()) => {
                self.history.extend(turn.phase.messages());
                self.reporter.operation(
                    turn.phase.tag,
                    Lifecycle::Completed,
                    "Tool batch completed",
                );
                self.launch_provider(turn.provider(), None);
            }
        }
    }

    fn stop<P: Into<CleanupWork>>(
        &mut self,
        turn: Turn<P>,
        outcome: TurnOutcome,
        history: HistoryDisposition,
    ) {
        let turn = turn.stopping(outcome, history);
        if matches!(turn.phase.work, CleanupWork::Provider(_))
            && !matches!(turn.phase.outcome, TurnOutcome::Completed)
        {
            self.preserve_completed_content();
        }
        if matches!(turn.phase.outcome, TurnOutcome::Cancelled) {
            self.reporter.turn(turn.id, Lifecycle::Cancelling, None);
        }
        let scope = turn.scope.clone();
        let id = turn.id;
        scope.cancel.cancel();
        self.turn = TurnState::Stopping(turn);
        let actor = self.actor_ref.clone();
        self.dependency.runtime.scope.tasks.spawn(async move {
            scope.finish().await;
            let _ = actor.send_message(Message::CleanupFinished { turn: id });
        });
    }

    pub(crate) async fn cleanup_finished(&mut self, id: TurnId) {
        match std::mem::take(&mut self.turn) {
            TurnState::Stopping(stopping) if stopping.id == id => self.finish_turn(stopping).await,
            obsolete => self.turn = obsolete,
        }
    }

    async fn finish_turn(&mut self, stopping: Turn<Cleanup>) {
        let id = stopping.id;
        self.finish_history(&stopping);
        if matches!(stopping.phase.outcome, TurnOutcome::Cancelled) {
            self.reporter.operation(
                stopping.phase.work.tag(),
                Lifecycle::Cancelled,
                "Operation cancelled after cleanup",
            );
        }
        self.reporter.turn(
            id,
            stopping.phase.outcome.lifecycle(),
            stopping.phase.outcome.detail(),
        );
        self.stream_processor.clear();
        self.change_state(State::Ready);
        match std::mem::replace(&mut self.role, SessionRole::Interactive) {
            SessionRole::Worker(reply) => {
                let result = match stopping.phase.outcome {
                    TurnOutcome::Completed => Ok(self
                        .history
                        .last()
                        .map(llm::Message::text)
                        .unwrap_or_default()),
                    TurnOutcome::Cancelled => Err(WorkerFailure::Cancelled),
                    TurnOutcome::Failed(failure) => Err(WorkerFailure::Turn(failure)),
                };
                let _ = reply.send(result);
                self.actor_ref.stop(None);
            }
            SessionRole::Interactive => {
                if matches!(stopping.phase.history, HistoryDisposition::Clear) {
                    self.clear_session().await;
                }
                if let Some(follow_up) = self.queue.pop_front() {
                    self.begin(follow_up);
                }
            }
        }
    }

    pub(crate) async fn interrupt(&mut self, history: HistoryDisposition) {
        self.cancel_queue();
        match std::mem::take(&mut self.turn) {
            TurnState::Idle => {
                if matches!(history, HistoryDisposition::Clear) {
                    self.clear_session().await;
                }
            }
            TurnState::Provider(turn) => self.stop(turn, TurnOutcome::Cancelled, history),
            TurnState::Tools(turn) => self.stop(turn, TurnOutcome::Cancelled, history),
            TurnState::Stopping(mut stopping) => {
                if matches!(history, HistoryDisposition::Clear) {
                    stopping.phase.history = history;
                }
                self.turn = TurnState::Stopping(stopping);
            }
        }
    }

    fn finish_history(&mut self, turn: &Turn<Cleanup>) {
        if let CleanupWork::Tools(batch) = &turn.phase.work {
            self.history.extend(batch.messages());
            for operation in batch.pending_operations() {
                self.reporter.operation(
                    Tag {
                        turn: turn.id,
                        operation,
                    },
                    Lifecycle::Cancelled,
                    "Operation stopped; inspect any uncertain effects",
                );
            }
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

    fn cancel_queue(&mut self) {
        for follow_up in self.queue.drain(..) {
            self.reporter.turn(
                follow_up.id,
                Lifecycle::Cancelled,
                Some("Queued follow-up cancelled".into()),
            );
        }
    }
    async fn clear_session(&mut self) {
        self.clear_history().await;
        self.reporter.send(ActorToTuiPacket::CommandResult(
            Command::Clear,
            "History cleared".into(),
        ));
    }

    #[cfg(test)]
    pub(crate) fn visible_history(&self) -> Vec<llm::Message> {
        let mut history = self.history.clone();
        if let Some(batch) = self.turn.batch() {
            history.extend(batch.messages());
        }
        history
    }

    pub(crate) async fn shutdown(&mut self) {
        let turn = match std::mem::take(&mut self.turn) {
            TurnState::Provider(turn) => {
                Some(turn.stopping(TurnOutcome::Cancelled, HistoryDisposition::Retain))
            }
            TurnState::Tools(turn) => {
                Some(turn.stopping(TurnOutcome::Cancelled, HistoryDisposition::Retain))
            }
            TurnState::Stopping(turn) => Some(turn),
            TurnState::Idle => None,
        };
        if let Some(turn) = &turn {
            turn.scope.finish().await;
            self.finish_history(turn);
        }
        self.dependency.runtime.scope.finish().await;
        self.cancel_queue();
        if let Some(turn) = turn {
            self.reporter.operation(
                turn.phase.work.tag(),
                Lifecycle::Cancelled,
                "Actor stopped after cleanup",
            );
            self.reporter.turn(
                turn.id,
                Lifecycle::Cancelled,
                Some("Actor stopped after cleanup".into()),
            );
        }
        if let SessionRole::Worker(reply) =
            std::mem::replace(&mut self.role, SessionRole::Interactive)
        {
            let _ = reply.send(Err(WorkerFailure::Stopped));
        }
    }

    pub(crate) async fn command(&mut self, command: Command) {
        match command {
            Command::Clear => self.interrupt(HistoryDisposition::Clear).await,
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
