use super::*;
use crate::{
    runtime::Workspace, stream_processor::ProcessedItem, tool_call::ToolCall, turn::ToolBatch,
};
use clients::failure::FailureKind;
use common_models::runtime_ids::OperationId;
use tools::tool_defs::{ToolEffect, ToolId, ToolInvocation};

fn machine() -> TurnMachine {
    TurnMachine::new(ExecutionScope::default())
}

fn provider(machine: &TurnMachine) -> &ProviderRun {
    match &machine.state {
        SessionState::Running(Session {
            state: TurnState::Provider(turn),
            ..
        }) => &turn.phase,
        _ => panic!("expected a provider request"),
    }
}

fn start(machine: &mut TurnMachine) -> Tag {
    machine.transition(SessionEvent::Start(FollowUp::new(Some("task".into()))));
    provider(machine).tag
}

fn response(
    machine: &mut TurnMachine,
    tag: Tag,
    result: Result<AcceptedResponse, Failure>,
) -> Vec<Effect> {
    machine.transition(SessionEvent::Provider {
        tag,
        update: ProviderUpdate::Finished(result),
    })
}

fn complete_response() -> AcceptedResponse {
    AcceptedResponse::Complete(llm::Message {
        role: llm::Role::Assistant,
        content: vec![llm::ContentBlock::MessageBlock {
            text: "done".into(),
            phase: None,
        }],
    })
}

struct ToolBatchFixture {
    tag: Tag,
    jobs: Vec<ToolJob>,
}

fn begin_tools(machine: &mut TurnMachine, provider: Tag) -> ToolBatchFixture {
    let items = ["first", "second", "third"].map(|id| {
        ProcessedItem::Tool(ToolCall {
            id: ToolId {
                id: id.to_owned().try_into().unwrap(),
                call_id: None,
            },
            name: "read".to_owned().try_into().unwrap(),
            input: Default::default(),
        })
    });
    let batch = ToolBatch::new(provider.turn, items.into());
    let tag = batch.tag;
    let jobs = batch.jobs();
    response(machine, provider, Ok(AcceptedResponse::Tools(batch)));
    ToolBatchFixture { tag, jobs }
}

fn tool(machine: &mut TurnMachine, tag: Tag, event: ToolEvent) -> Vec<Effect> {
    machine.transition(SessionEvent::Tools {
        tag,
        event,
        revision: Workspace::new(1).revision(),
    })
}

fn success(job: &ToolJob) -> ToolResult {
    ToolResult {
        id: job.call.id.clone(),
        invocation: ToolInvocation {
            name: job.call.name.clone(),
            input: job.call.input.clone(),
            display: "read".into(),
        },
        outcome: Ok("read result".into()),
    }
}

fn complete_tool(machine: &mut TurnMachine, tag: Tag, job: &ToolJob) -> Vec<Effect> {
    tool(
        machine,
        tag,
        ToolEvent::Completed {
            operation: job.operation,
            result: success(job),
        },
    )
}

fn launches_provider(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::LaunchProvider { .. }))
}

#[test]
fn retries_replace_only_the_request_and_stop_at_the_budget() {
    let mut machine = machine();
    let initial = start(&mut machine);
    let mut tag = initial;
    for attempt in 1..=2 {
        let previous_scope = provider(&machine).scope.clone();
        let effects = response(
            &mut machine,
            tag,
            Err(Failure::new(FailureKind::Transport, "connection lost")),
        );
        let retry = provider(&machine);
        assert_eq!(retry.tag.turn, initial.turn);
        assert_ne!(retry.tag.operation, tag.operation);
        assert_eq!(retry.attempt, attempt);
        assert!(effects.iter().any(|effect| matches!(
            effect,
            Effect::LaunchProvider {
                previous: Some(_),
                ..
            }
        )));
        assert!(!effects.iter().any(|effect| matches!(
            effect,
            Effect::LaunchTools { .. } | Effect::AppendHistory(_)
        )));
        assert!(!previous_scope.cancel.is_cancelled());
        assert_eq!(retry.scope.tasks.len(), 0);
        let retry_tag = retry.tag;
        assert!(machine.provider_response(tag).is_none());
        assert!(response(&mut machine, tag, Ok(complete_response())).is_empty());
        assert_eq!(provider(&machine).tag, retry_tag);
        tag = retry_tag;
    }
    let effects = response(
        &mut machine,
        tag,
        Err(Failure::new(FailureKind::Transport, "connection lost")),
    );
    assert!(!launches_provider(&effects));
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::PreserveCompletedContent))
    );
    assert!(matches!(
        machine.state,
        SessionState::Running(Session {
            state: TurnState::Stopping(_),
            ..
        })
    ));
    let effects = machine.transition(SessionEvent::CleanupFinished(initial.turn));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Report(ActorToTuiPacket::TurnChanged {
            state: Lifecycle::Failed,
            ..
        })
    )));
    assert!(matches!(
        machine.state,
        SessionState::Running(Session {
            state: TurnState::Idle,
            ..
        })
    ));
}

#[test]
fn accepted_tools_are_committed_before_continuing_and_are_not_retried() {
    let mut machine = machine();
    let initial = start(&mut machine);
    let ToolBatchFixture { tag: batch, jobs } = begin_tools(&mut machine, initial);
    for job in &jobs {
        complete_tool(&mut machine, batch, job);
    }
    let effects = tool(&mut machine, batch, ToolEvent::Finished(Ok(())));
    let history = effects
        .iter()
        .position(|effect| matches!(effect, Effect::AppendHistory(_)))
        .unwrap();
    let request = effects
        .iter()
        .position(|effect| matches!(effect, Effect::LaunchProvider { .. }))
        .unwrap();
    assert!(history < request);
    let tag = provider(&machine).tag;
    assert_eq!(tag.turn, initial.turn);
    assert_eq!(provider(&machine).attempt, 0);
    let effects = response(
        &mut machine,
        tag,
        Err(Failure::new(FailureKind::Transport, "lost response")),
    );
    assert!(launches_provider(&effects));
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        Effect::LaunchTools { .. } | Effect::AppendHistory(_)
    )));
    assert!(tool(&mut machine, batch, ToolEvent::Finished(Ok(()))).is_empty());
    assert!(complete_tool(&mut machine, batch, &jobs[0]).is_empty());
}

#[test]
fn clear_upgrades_pending_cleanup_and_precedes_the_next_queued_prompt() {
    let mut machine = machine();
    let tag = start(&mut machine);
    let cancelled = FollowUp::new(Some("cancel this".into()));
    let cancelled_id = cancelled.id;
    machine.transition(SessionEvent::Start(cancelled));
    let effects = machine.transition(SessionEvent::Interrupt(HistoryDisposition::Retain));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Cleanup { .. }))
            .count(),
        1
    );
    assert!(effects.iter().any(|effect| matches!(effect,
        Effect::Report(ActorToTuiPacket::TurnChanged { turn_id, state: Lifecycle::Cancelled, .. }) if *turn_id == cancelled_id
    )));
    assert!(
        machine
            .transition(SessionEvent::Interrupt(HistoryDisposition::Clear))
            .is_empty()
    );
    assert!(
        machine
            .transition(SessionEvent::Interrupt(HistoryDisposition::Retain))
            .is_empty()
    );
    let next = FollowUp::new(Some("new task".into()));
    let next_id = next.id;
    let effects = machine.transition(SessionEvent::Start(next));
    assert!(!launches_provider(&effects));
    assert!(
        machine
            .transition(SessionEvent::CleanupFinished(TurnId::new()))
            .is_empty()
    );
    assert!(matches!(
        machine.state,
        SessionState::Running(Session {
            state: TurnState::Stopping(_),
            ..
        })
    ));
    let effects = machine.transition(SessionEvent::CleanupFinished(tag.turn));
    let clear = effects
        .iter()
        .position(|effect| matches!(effect, Effect::ClearHistory))
        .unwrap();
    let prompt = effects
        .iter()
        .position(|effect| matches!(effect, Effect::AppendHistory(_)))
        .unwrap();
    let request = effects
        .iter()
        .position(|effect| matches!(effect, Effect::LaunchProvider { .. }))
        .unwrap();
    assert!(clear < prompt && prompt < request);
    assert_eq!(provider(&machine).tag.turn, next_id);
    assert!(
        machine
            .transition(SessionEvent::CleanupFinished(tag.turn))
            .is_empty()
    );
}

#[test]
fn stopping_accepts_matching_tool_results_once_and_preserves_uncertain_work() {
    let mut machine = machine();
    let initial = start(&mut machine);
    let ToolBatchFixture { tag: batch, jobs } = begin_tools(&mut machine, initial);
    for job in &jobs[..2] {
        tool(
            &mut machine,
            batch,
            ToolEvent::Started {
                operation: job.operation,
                effect: ToolEffect::Read,
                revision: None,
                display: "read".into(),
            },
        );
    }
    machine.transition(SessionEvent::Interrupt(HistoryDisposition::Retain));
    let wrong_tag = Tag {
        operation: OperationId::new(),
        ..batch
    };
    assert!(complete_tool(&mut machine, wrong_tag, &jobs[0]).is_empty());
    assert!(
        tool(
            &mut machine,
            batch,
            ToolEvent::Completed {
                operation: jobs[0].operation,
                result: success(&jobs[1])
            }
        )
        .is_empty()
    );
    let effects = complete_tool(&mut machine, batch, &jobs[0]);
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::UpdateContext { .. }))
            .count(),
        1
    );
    assert!(complete_tool(&mut machine, batch, &jobs[0]).is_empty());
    assert!(tool(&mut machine, batch, ToolEvent::Finished(Ok(()))).is_empty());
    let effects = machine.transition(SessionEvent::CleanupFinished(initial.turn));
    let messages = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::AppendHistory(messages) => Some(messages),
            _ => None,
        })
        .unwrap();
    let outputs = &messages[1].content;
    assert!(
        matches!(&outputs[0], llm::ContentBlock::ToolResult { content, is_error: None, .. } if content == "read result")
    );
    assert!(
        matches!(&outputs[1], llm::ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("completion is unknown"))
    );
    assert!(
        matches!(&outputs[2], llm::ContentBlock::ToolResult { content, is_error: Some(true), .. } if content.contains("Not executed"))
    );
    assert!(!launches_provider(&effects));
}

#[test]
fn context_failure_stops_continuation_without_losing_successful_tool_output() {
    let mut machine = machine();
    let initial = start(&mut machine);
    let ToolBatchFixture { tag: batch, jobs } = begin_tools(&mut machine, initial);
    for job in &jobs {
        complete_tool(&mut machine, batch, job);
    }
    machine.feedback(EffectOutcome::ContextFailed {
        tag: batch,
        failure: Failure::new(FailureKind::Tool, "context hook failed"),
    });
    let effects = tool(&mut machine, batch, ToolEvent::Finished(Ok(())));
    assert!(!launches_provider(&effects));
    assert!(
        matches!(&machine.state, SessionState::Running(Session { state: TurnState::Stopping(turn), .. })
            if matches!(&turn.phase.outcome, TurnOutcome::Failed(failure) if failure.message == "context hook failed")
        )
    );
    let effects = machine.transition(SessionEvent::CleanupFinished(initial.turn));
    assert!(effects.iter().any(|effect| matches!(effect, Effect::AppendHistory(messages)
        if messages[1].content.iter().all(|content| matches!(content, llm::ContentBlock::ToolResult { is_error: None, .. }))
    )));
}

#[test]
fn completed_worker_replies_after_cleanup_and_does_not_start_queued_work() {
    let mut machine = machine();
    let (reply, _receive) = tokio::sync::oneshot::channel();
    machine.transition(SessionEvent::StartWorker(reply.into()));
    let tag = provider(&machine).tag;
    let (second, _receive) = tokio::sync::oneshot::channel();
    let effects = machine.transition(SessionEvent::StartWorker(second.into()));
    assert!(matches!(
        effects.as_slice(),
        [Effect::ReplyWorker {
            outcome: WorkerOutcome::Failed(WorkerFailure::AlreadyRunning),
            ..
        }]
    ));
    machine.transition(SessionEvent::Start(FollowUp::new(Some("queued".into()))));
    let effects = response(&mut machine, tag, Ok(complete_response()));
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::ReplyWorker { .. } | Effect::StopActor))
    );
    let effects = machine.transition(SessionEvent::CleanupFinished(tag.turn));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::ReplyWorker {
            outcome: WorkerOutcome::Completed,
            ..
        }
    )));
    assert!(matches!(effects.last(), Some(Effect::StopActor)));
    assert!(!launches_provider(&effects));
}

#[test]
fn shutdown_waits_for_resources_before_publishing_cancellation_or_resolving_worker() {
    let mut machine = machine();
    let (reply, _receive) = tokio::sync::oneshot::channel();
    machine.transition(SessionEvent::StartWorker(reply.into()));
    let tag = provider(&machine).tag;
    machine.transition(SessionEvent::Start(FollowUp::new(Some("queued".into()))));
    let effects = machine.transition(Event::Shutdown);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Shutdown(ShutdownScope::Turn(_))]
    ));
    assert!(
        machine
            .transition(SessionEvent::CleanupFinished(tag.turn))
            .is_empty()
    );
    assert!(
        machine
            .transition(SessionEvent::Start(FollowUp::new(None)))
            .is_empty()
    );
    assert!(machine.transition(Event::Shutdown).is_empty());
    let effects = machine.feedback(EffectOutcome::ShutdownFinished);
    assert!(effects.iter().any(|effect| matches!(effect,
        Effect::Report(ActorToTuiPacket::TurnChanged { turn_id, state: Lifecycle::Cancelled, .. }) if *turn_id == tag.turn
    )));
    assert!(matches!(
        effects.last(),
        Some(Effect::ReplyWorker {
            outcome: WorkerOutcome::Failed(WorkerFailure::Stopped),
            ..
        })
    ));
    assert!(!launches_provider(&effects));
    assert!(matches!(machine.state, SessionState::Stopped));
    assert!(machine.provider_response(tag).is_none());
    assert!(
        machine
            .transition(SessionEvent::Start(FollowUp::new(None)))
            .is_empty()
    );
    assert!(machine.transition(Event::ShutdownFinished).is_empty());
}

#[test]
fn idle_shutdown_drains_the_session_and_stays_stopped() {
    let mut machine = machine();
    assert!(machine.feedback(EffectOutcome::ShutdownFinished).is_empty());
    let effects = machine.transition(Event::Shutdown);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Shutdown(ShutdownScope::Session)]
    ));
    assert!(machine.feedback(EffectOutcome::ShutdownFinished).is_empty());
    assert!(matches!(machine.state, SessionState::Stopped));
    assert!(machine.feedback(EffectOutcome::Applied).is_empty());
    assert!(machine.transition(Event::Shutdown).is_empty());
    assert!(
        machine
            .transition(SessionEvent::Start(FollowUp::new(None)))
            .is_empty()
    );
    assert!(matches!(machine.state, SessionState::Stopped));
}
