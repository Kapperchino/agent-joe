use crate::{
    context::RequestMode,
    runtime::WorkspaceRevision,
    scheduler::ToolEvent,
    stream_processor::StreamNextStep,
    turn::{
        AcceptedResponse, Cleanup, CleanupWork, Continuation, FollowUp, HistoryDisposition,
        ProviderRun, ResponseState, Tag, ToolJob, Turn, TurnOutcome, TurnState, tool_failure,
    },
    worker::WorkerFailure,
};
use clients::{failure::Failure, llm};
use common_models::{
    interaction::QuestionGate,
    runtime_ids::TurnId,
    tui_models::{ActorToTuiPacket, Lifecycle, State},
};
use ractor::RpcReplyPort;
use std::collections::VecDeque;
use tools::tool_defs::ToolResult;
use utils::execution::ExecutionScope;

type WorkerReply = RpcReplyPort<Result<String, WorkerFailure>>;

pub(crate) enum Event {
    Session(SessionEvent),
    StopRequested,
    Shutdown,
    ShutdownFinished,
}

impl From<SessionEvent> for Event {
    fn from(event: SessionEvent) -> Self {
        Self::Session(event)
    }
}

pub(crate) enum SessionEvent {
    QuestionsChanged(QuestionGate),
    Steer(FollowUp),
    Start(FollowUp),
    StartWorker(WorkerReply),
    Provider {
        tag: Tag,
        update: ProviderUpdate,
    },
    Tools {
        tag: Tag,
        event: ToolEvent,
        revision: WorkspaceRevision,
    },
    ContextFailed {
        tag: Tag,
        failure: Failure,
    },
    Interrupt(HistoryDisposition),
    CleanupFinished(TurnId),
}

pub(crate) enum ProviderUpdate {
    Progress(StreamNextStep),
    Finished(Result<AcceptedResponse, Failure>),
}

impl ProviderUpdate {
    fn for_mode(self, mode: RequestMode) -> Self {
        match (mode, self) {
            (
                RequestMode::SingleResponse,
                Self::Progress(StreamNextStep::ToolUse | StreamNextStep::Refused),
            ) => Self::Finished(Err(Failure::new(
                clients::failure::FailureKind::InvalidInput,
                "Expected a completed text response without tools or refusal",
            ))),
            (RequestMode::SingleResponse, Self::Finished(Ok(response))) => {
                Self::Finished(response.text_only())
            }
            (_, update) => update,
        }
    }
}

pub(crate) enum Effect {
    QueueInput(FollowUp),
    BeginTurn(FollowUp),
    AppendHistory(Vec<llm::Message>),
    ClearHistory,
    ClearStream,
    PreserveCompletedContent,
    ChangeState(State),
    Report(ActorToTuiPacket),
    LaunchProvider {
        run: ProviderRun,
        owner: ExecutionScope,
        previous: Option<ExecutionScope>,
    },
    LaunchTools {
        jobs: Vec<ToolJob>,
        tag: Tag,
        scope: ExecutionScope,
    },
    UpdateContext {
        tag: Tag,
        result: ToolResult,
    },
    Cleanup {
        turn: TurnId,
        scope: ExecutionScope,
    },
    Shutdown(ShutdownScope),
    ReplyWorker {
        reply: WorkerReply,
        outcome: WorkerOutcome,
    },
    StopActor,
}

impl Effect {
    fn turn(id: TurnId, state: Lifecycle, detail: Option<String>) -> Self {
        Self::Report(ActorToTuiPacket::TurnChanged {
            turn_id: id,
            state,
            detail,
        })
    }

    fn operation(tag: Tag, state: Lifecycle, detail: impl Into<String>) -> Self {
        Self::Report(ActorToTuiPacket::OperationChanged {
            turn_id: tag.turn,
            operation_id: tag.operation,
            state,
            detail: detail.into(),
        })
    }
}

pub(crate) enum EffectOutcome {
    Applied,
    ContextFailed { tag: Tag, failure: Failure },
    ShutdownFinished,
}

pub(crate) enum WorkerOutcome {
    Completed,
    Failed(WorkerFailure),
}

pub(crate) enum ShutdownScope {
    Session,
    Turn(ExecutionScope),
}

enum SessionRole {
    Interactive,
    Worker(WorkerReply),
}

#[derive(Default)]
enum SessionState {
    Running(Session),
    Closing(Session),
    ShuttingDown(Shutdown),
    #[default]
    Stopped,
}

pub(crate) struct TurnMachine {
    state: SessionState,
}

struct Session {
    questions: QuestionGate,
    mode: RequestMode,
    state: TurnState,
    queue: VecDeque<FollowUp>,
    role: SessionRole,
    scope: ExecutionScope,
}

struct Shutdown {
    work: ShutdownWork,
    queue: VecDeque<FollowUp>,
    role: SessionRole,
}

enum ShutdownWork {
    Idle,
    Turn(Turn<Cleanup>),
}

impl TurnMachine {
    pub fn new(scope: ExecutionScope, mode: RequestMode) -> Self {
        Self {
            state: SessionState::Running(Session {
                questions: QuestionGate::Open,
                mode,
                state: TurnState::Idle,
                queue: VecDeque::new(),
                role: SessionRole::Interactive,
                scope,
            }),
        }
    }

    pub fn provider_response(&self, tag: Tag) -> Option<ResponseState> {
        match &self.state {
            SessionState::Running(Session {
                state: TurnState::Provider(turn),
                ..
            }) if turn.phase.tag == tag => Some(turn.phase.response),
            _ => None,
        }
    }

    pub fn transition(&mut self, event: impl Into<Event>) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.state = match (std::mem::take(&mut self.state), event.into()) {
            (SessionState::Running(mut session), Event::Session(event)) => {
                session.transition(event, &mut effects);
                SessionState::Running(session)
            }
            (SessionState::Running(mut session), Event::StopRequested) => {
                session.transition(
                    SessionEvent::Interrupt(HistoryDisposition::Retain),
                    &mut effects,
                );
                match session.state {
                    TurnState::Stopping(_) => SessionState::Closing(session),
                    _ => SessionState::ShuttingDown(session.shutdown(&mut effects)),
                }
            }
            (
                SessionState::Closing(mut session),
                Event::Session(
                    event @ (SessionEvent::Tools { .. } | SessionEvent::ContextFailed { .. }),
                ),
            ) => {
                session.transition(event, &mut effects);
                SessionState::Closing(session)
            }
            (SessionState::Closing(session), Event::Session(SessionEvent::CleanupFinished(id)))
                if matches!(&session.state, TurnState::Stopping(turn) if turn.id == id) =>
            {
                SessionState::ShuttingDown(session.shutdown(&mut effects))
            }
            (SessionState::Running(session) | SessionState::Closing(session), Event::Shutdown) => {
                SessionState::ShuttingDown(session.shutdown(&mut effects))
            }
            (SessionState::ShuttingDown(shutdown), Event::ShutdownFinished) => {
                shutdown.finish(&mut effects);
                effects.push(Effect::StopActor);
                SessionState::Stopped
            }
            (state, Event::Session(SessionEvent::StartWorker(reply))) => {
                effects.push(Effect::ReplyWorker {
                    reply,
                    outcome: WorkerOutcome::Failed(WorkerFailure::Stopped),
                });
                state
            }
            (state, _) => state,
        };
        effects
    }

    pub fn feedback(&mut self, outcome: EffectOutcome) -> Vec<Effect> {
        match outcome {
            EffectOutcome::Applied => Vec::new(),
            EffectOutcome::ContextFailed { tag, failure } => {
                self.transition(SessionEvent::ContextFailed { tag, failure })
            }
            EffectOutcome::ShutdownFinished => self.transition(Event::ShutdownFinished),
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(
            &self.state,
            SessionState::Running(Session {
                state: TurnState::Idle | TurnState::Waiting(_),
                ..
            })
        )
    }

    pub fn batch(&self) -> Option<&crate::turn::ToolBatch> {
        match &self.state {
            SessionState::Running(session) | SessionState::Closing(session) => {
                session.state.batch()
            }
            SessionState::ShuttingDown(_) | SessionState::Stopped => None,
        }
    }
}

impl Session {
    fn transition(&mut self, event: SessionEvent, effects: &mut Vec<Effect>) {
        match event {
            SessionEvent::QuestionsChanged(questions) => {
                let previous = std::mem::replace(&mut self.questions, questions);
                match (&self.state, previous, questions) {
                    (
                        TurnState::Idle | TurnState::Waiting(_),
                        QuestionGate::Required,
                        QuestionGate::Open,
                    ) => {
                        let input = self
                            .queue
                            .pop_front()
                            .unwrap_or_else(|| FollowUp::new(None));
                        self.begin(input, effects);
                    }
                    _ => {}
                }
            }
            SessionEvent::Steer(follow_up) => {
                let id = follow_up.id;
                self.transition(SessionEvent::Interrupt(HistoryDisposition::Retain), effects);
                effects.push(Effect::Report(ActorToTuiPacket::InputAccepted {
                    turn_id: id,
                    kind: common_models::tui_models::InputKind::Steering,
                }));
                self.transition(SessionEvent::Start(follow_up), effects);
            }
            SessionEvent::Start(follow_up) => {
                effects.push(Effect::QueueInput(follow_up.clone()));
                match (&self.state, self.questions) {
                    (TurnState::Idle, QuestionGate::Open) => self.begin(follow_up, effects),
                    _ => {
                        let id = follow_up.id;
                        self.queue.push_back(follow_up);
                        if matches!(
                            (&self.state, self.questions),
                            (TurnState::Idle, QuestionGate::Required)
                        ) {
                            self.state = TurnState::Waiting(id);
                            effects.push(Effect::turn(
                                id,
                                Lifecycle::WaitingForInput,
                                Some(
                                    "Required question pending; follow-up saved in the queue"
                                        .into(),
                                ),
                            ));
                        }
                        effects.push(Effect::Report(ActorToTuiPacket::Queued {
                            turn_id: id,
                            position: self.queue.len(),
                        }));
                    }
                }
            }
            SessionEvent::StartWorker(reply) => match (&self.state, &self.role) {
                (TurnState::Idle, SessionRole::Interactive) => {
                    self.role = SessionRole::Worker(reply);
                    self.begin(FollowUp::new(None), effects);
                }
                _ => effects.push(Effect::ReplyWorker {
                    reply,
                    outcome: WorkerOutcome::Failed(WorkerFailure::AlreadyRunning),
                }),
            },
            SessionEvent::Provider { tag, update } => match std::mem::take(&mut self.state) {
                TurnState::Provider(mut turn) if turn.phase.tag == tag => {
                    match update.for_mode(self.mode) {
                        ProviderUpdate::Progress(step) => {
                            turn.phase.response = turn.phase.response.advance(step);
                            self.state = TurnState::Provider(turn);
                        }
                        ProviderUpdate::Finished(Ok(response)) => {
                            self.accept_response(turn, response, effects);
                        }
                        ProviderUpdate::Finished(Err(failure)) => {
                            self.provider_failed(turn, failure, effects);
                        }
                    }
                }
                obsolete => self.state = obsolete,
            },
            SessionEvent::Tools {
                tag,
                event,
                revision,
            } => self.tool_event(tag, event, revision, effects),
            SessionEvent::ContextFailed { tag, failure } => {
                if let Some(tools) = self.state.tools_mut(tag) {
                    tools.batch.continuation = Continuation::Stop(failure);
                }
            }
            SessionEvent::Interrupt(history) => {
                self.cancel_queue(effects);
                if matches!(history, HistoryDisposition::Clear) {
                    self.questions = QuestionGate::Open;
                }
                match std::mem::take(&mut self.state) {
                    TurnState::Idle => {
                        if matches!(history, HistoryDisposition::Clear) {
                            effects.push(Effect::ClearHistory);
                        }
                    }
                    TurnState::Provider(turn) => {
                        self.stop(turn, TurnOutcome::Cancelled, history, effects);
                    }
                    TurnState::Waiting(id) => {
                        effects.push(Effect::turn(
                            id,
                            Lifecycle::Cancelled,
                            Some(
                                "Waiting turn cancelled; unanswered questions remain saved".into(),
                            ),
                        ));
                        if matches!(history, HistoryDisposition::Clear) {
                            effects.push(Effect::ClearHistory);
                        }
                    }
                    TurnState::Tools(turn) => {
                        self.stop(turn, TurnOutcome::Cancelled, history, effects);
                    }
                    TurnState::Stopping(mut turn) => {
                        if matches!(turn.phase.outcome, TurnOutcome::WaitingForInput) {
                            turn.phase.outcome = TurnOutcome::Cancelled;
                        }
                        if matches!(history, HistoryDisposition::Clear) {
                            turn.phase.history = history;
                        }
                        self.state = TurnState::Stopping(turn);
                    }
                }
            }
            SessionEvent::CleanupFinished(id) => match std::mem::take(&mut self.state) {
                TurnState::Stopping(turn) if turn.id == id => self.finish_turn(turn, effects),
                obsolete => self.state = obsolete,
            },
        }
    }

    fn begin(&mut self, follow_up: FollowUp, effects: &mut Vec<Effect>) {
        let id = follow_up.id;
        effects.push(Effect::BeginTurn(follow_up));
        effects.push(Effect::Report(ActorToTuiPacket::InputAccepted {
            turn_id: id,
            kind: common_models::tui_models::InputKind::Active,
        }));
        self.launch_provider(Turn::new(id, self.scope.child()), None, effects);
    }

    fn launch_provider(
        &mut self,
        turn: Turn<ProviderRun>,
        previous: Option<ExecutionScope>,
        effects: &mut Vec<Effect>,
    ) {
        match self.questions {
            QuestionGate::Required => {
                self.stop(
                    turn,
                    TurnOutcome::WaitingForInput,
                    HistoryDisposition::Retain,
                    effects,
                );
            }
            QuestionGate::Open => {
                effects.extend([
                    Effect::ClearStream,
                    Effect::ChangeState(State::StreamStart),
                    Effect::turn(turn.id, Lifecycle::Running, None),
                    Effect::operation(turn.phase.tag, Lifecycle::Running, "Provider request"),
                    Effect::LaunchProvider {
                        run: turn.phase.clone(),
                        owner: turn.scope.clone(),
                        previous,
                    },
                ]);
                self.state = TurnState::Provider(turn);
            }
        }
    }

    fn accept_response(
        &mut self,
        turn: Turn<ProviderRun>,
        response: AcceptedResponse,
        effects: &mut Vec<Effect>,
    ) {
        effects.push(Effect::operation(
            turn.phase.tag,
            Lifecycle::Completed,
            "Provider response received",
        ));
        match response {
            AcceptedResponse::Compacted => {
                self.stop(
                    turn,
                    TurnOutcome::Completed,
                    HistoryDisposition::Retain,
                    effects,
                );
            }
            AcceptedResponse::Tools(batch) => {
                effects.extend([
                    Effect::turn(turn.id, Lifecycle::WaitingForTools, None),
                    Effect::operation(batch.tag, Lifecycle::WaitingForTools, "Tool batch"),
                    Effect::ChangeState(State::ToolStart),
                    Effect::LaunchTools {
                        jobs: batch.jobs(),
                        tag: batch.tag,
                        scope: turn.scope.clone(),
                    },
                ]);
                self.state = TurnState::Tools(turn.map(|_| batch));
            }
            AcceptedResponse::Complete(message) => {
                effects.push(Effect::AppendHistory(vec![message]));
                self.stop(
                    turn,
                    TurnOutcome::Completed,
                    HistoryDisposition::Retain,
                    effects,
                );
            }
        }
    }

    fn provider_failed(
        &mut self,
        turn: Turn<ProviderRun>,
        failure: Failure,
        effects: &mut Vec<Effect>,
    ) {
        effects.push(Effect::operation(
            turn.phase.tag,
            Lifecycle::Failed,
            failure.to_string(),
        ));
        if self.mode != RequestMode::SingleResponse && failure.retryable() && turn.phase.attempt < 2
        {
            let previous = turn.phase.scope.clone();
            let run = ProviderRun::new(turn.id, turn.scope.child(), turn.phase.attempt + 1);
            effects.push(Effect::turn(
                turn.id,
                Lifecycle::Running,
                Some(format!(
                    "{failure}. Retrying the interrupted provider request."
                )),
            ));
            self.launch_provider(turn.map(|_| run), Some(previous), effects);
        } else {
            self.stop(
                turn,
                TurnOutcome::Failed(failure),
                HistoryDisposition::Retain,
                effects,
            );
        }
    }

    fn tool_event(
        &mut self,
        tag: Tag,
        event: ToolEvent,
        revision: WorkspaceRevision,
        effects: &mut Vec<Effect>,
    ) {
        match event {
            ToolEvent::Started {
                operation,
                effect,
                revision,
                display,
            } => {
                if let Some(tools) = self.state.tools_mut(tag)
                    && tools.batch.start(operation).is_some()
                {
                    effects.push(Effect::Report(ActorToTuiPacket::ToolUse(vec![display])));
                    let detail = match revision {
                        Some(revision) => {
                            format!("{effect:?} tool at workspace revision {revision}")
                        }
                        None => format!("{effect:?} worker"),
                    };
                    effects.push(Effect::operation(
                        Tag { operation, ..tag },
                        Lifecycle::Running,
                        detail,
                    ));
                }
            }
            ToolEvent::Completed { operation, result } => {
                if let Some(tools) = self.state.tools_mut(tag)
                    && let Some(result) = tools.batch.complete(operation, result)
                {
                    if let Continuation::Stop(failure) = tools.failures.record(&result, revision) {
                        tools.batch.continuation = Continuation::Stop(failure);
                    }
                    let completion = match &result.outcome {
                        Ok(_) => Effect::operation(
                            Tag { operation, ..tag },
                            Lifecycle::Completed,
                            result.invocation.display.clone(),
                        ),
                        Err(failure) => Effect::operation(
                            Tag { operation, ..tag },
                            Lifecycle::Failed,
                            failure.to_string(),
                        ),
                    };
                    effects.push(Effect::UpdateContext { tag, result });
                    effects.push(completion);
                }
            }
            ToolEvent::Finished(result) => match std::mem::take(&mut self.state) {
                TurnState::Tools(turn) if turn.phase.tag == tag => {
                    effects.push(Effect::ChangeState(State::ToolStop));
                    let continuation = match result {
                        Ok(()) => turn.phase.continuation.clone(),
                        Err(failure) => Continuation::Stop(tool_failure(&failure)),
                    };
                    match continuation {
                        Continuation::Stop(failure) => {
                            effects.push(Effect::operation(
                                tag,
                                Lifecycle::Failed,
                                failure.to_string(),
                            ));
                            self.stop(
                                turn,
                                TurnOutcome::Failed(failure),
                                HistoryDisposition::Retain,
                                effects,
                            );
                        }
                        Continuation::Continue => {
                            effects.push(Effect::AppendHistory(turn.phase.messages().into()));
                            effects.push(Effect::operation(
                                tag,
                                Lifecycle::Completed,
                                "Tool batch completed",
                            ));
                            self.launch_provider(turn.provider(), None, effects);
                        }
                    }
                }
                obsolete => self.state = obsolete,
            },
        }
    }

    fn stop<P: Into<CleanupWork>>(
        &mut self,
        turn: Turn<P>,
        outcome: TurnOutcome,
        history: HistoryDisposition,
        effects: &mut Vec<Effect>,
    ) {
        let turn = turn.stopping(outcome, history);
        if matches!(turn.phase.work, CleanupWork::Provider(_))
            && matches!(
                turn.phase.outcome,
                TurnOutcome::Cancelled | TurnOutcome::Failed(_)
            )
        {
            effects.push(Effect::PreserveCompletedContent);
        }
        if matches!(turn.phase.outcome, TurnOutcome::Cancelled) {
            effects.push(Effect::turn(turn.id, Lifecycle::Cancelling, None));
        }
        effects.push(Effect::Cleanup {
            turn: turn.id,
            scope: turn.scope.clone(),
        });
        self.state = TurnState::Stopping(turn);
    }

    fn finish_history(turn: &Turn<Cleanup>, effects: &mut Vec<Effect>) {
        if let CleanupWork::Tools(batch) = &turn.phase.work {
            effects.push(Effect::AppendHistory(batch.messages().into()));
            for operation in batch.pending_operations() {
                effects.push(Effect::operation(
                    Tag {
                        turn: turn.id,
                        operation,
                    },
                    Lifecycle::Cancelled,
                    "Operation stopped; inspect any uncertain effects",
                ));
            }
        }
    }

    fn finish_turn(&mut self, turn: Turn<Cleanup>, effects: &mut Vec<Effect>) {
        Self::finish_history(&turn, effects);
        if matches!(turn.phase.outcome, TurnOutcome::Cancelled) {
            effects.push(Effect::operation(
                turn.phase.work.tag(),
                Lifecycle::Cancelled,
                "Operation cancelled after cleanup",
            ));
        }
        effects.extend([
            Effect::turn(
                turn.id,
                turn.phase.outcome.lifecycle(),
                turn.phase.outcome.detail(),
            ),
            Effect::ClearStream,
            Effect::ChangeState(State::Ready),
        ]);
        match std::mem::replace(&mut self.role, SessionRole::Interactive) {
            SessionRole::Worker(reply) => {
                let outcome = match turn.phase.outcome {
                    TurnOutcome::WaitingForInput => WorkerOutcome::Failed(WorkerFailure::Cancelled),
                    TurnOutcome::Completed => WorkerOutcome::Completed,
                    TurnOutcome::Cancelled => WorkerOutcome::Failed(WorkerFailure::Cancelled),
                    TurnOutcome::Failed(failure) => {
                        WorkerOutcome::Failed(WorkerFailure::Turn(failure))
                    }
                };
                effects.push(Effect::ReplyWorker { reply, outcome });
                effects.push(Effect::StopActor);
            }
            SessionRole::Interactive => self.finish_interactive(turn, effects),
        }
    }

    fn finish_interactive(&mut self, turn: Turn<Cleanup>, effects: &mut Vec<Effect>) {
        if matches!(turn.phase.history, HistoryDisposition::Clear) {
            effects.push(Effect::ClearHistory);
        }
        let queued = match self.questions {
            QuestionGate::Open => self.queue.pop_front(),
            QuestionGate::Required => None,
        };
        match (turn.phase.outcome, self.questions, queued) {
            (TurnOutcome::WaitingForInput, QuestionGate::Required, _) => {
                self.state = TurnState::Waiting(turn.id);
            }
            (_, _, Some(input)) => self.begin(input, effects),
            (TurnOutcome::WaitingForInput, QuestionGate::Open, None) => {
                self.begin(FollowUp::new(None), effects);
            }
            (_, _, None) => {}
        }
    }

    fn cancel_queue(&mut self, effects: &mut Vec<Effect>) {
        effects.extend(self.queue.drain(..).map(|follow_up| {
            Effect::turn(
                follow_up.id,
                Lifecycle::Cancelled,
                Some("Queued follow-up cancelled".into()),
            )
        }));
    }

    fn shutdown(self, effects: &mut Vec<Effect>) -> Shutdown {
        let work = match self.state {
            TurnState::Idle | TurnState::Waiting(_) => ShutdownWork::Idle,
            TurnState::Provider(turn) => ShutdownWork::Turn(
                turn.stopping(TurnOutcome::Cancelled, HistoryDisposition::Retain),
            ),
            TurnState::Tools(turn) => ShutdownWork::Turn(
                turn.stopping(TurnOutcome::Cancelled, HistoryDisposition::Retain),
            ),
            TurnState::Stopping(turn) => ShutdownWork::Turn(turn),
        };
        let scope = match &work {
            ShutdownWork::Idle => ShutdownScope::Session,
            ShutdownWork::Turn(turn) => ShutdownScope::Turn(turn.scope.clone()),
        };
        effects.push(Effect::Shutdown(scope));
        Shutdown {
            work,
            queue: self.queue,
            role: self.role,
        }
    }
}

impl Shutdown {
    fn finish(self, effects: &mut Vec<Effect>) {
        for follow_up in self.queue {
            effects.push(Effect::turn(
                follow_up.id,
                Lifecycle::Cancelled,
                Some("Queued follow-up cancelled".into()),
            ));
        }
        if let ShutdownWork::Turn(turn) = self.work {
            Session::finish_history(&turn, effects);
            effects.push(Effect::operation(
                turn.phase.work.tag(),
                Lifecycle::Cancelled,
                "Actor stopped after cleanup",
            ));
            effects.push(Effect::turn(
                turn.id,
                Lifecycle::Cancelled,
                Some("Actor stopped after cleanup".into()),
            ));
        }
        if let SessionRole::Worker(reply) = self.role {
            effects.push(Effect::ReplyWorker {
                reply,
                outcome: WorkerOutcome::Failed(WorkerFailure::Stopped),
            });
        }
    }
}

#[cfg(test)]
#[path = "turn_machine_test.rs"]
mod tests;
