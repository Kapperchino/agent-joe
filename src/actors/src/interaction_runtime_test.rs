use super::*;
use commands::command::{Command, ResumeTarget};
use common_models::interaction::{Plan, PlanStep, PlanUpdate, StepState};

fn tool(name: &str, id: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolBlock {
        tool_id: ToolId {
            id: id.to_owned().try_into().unwrap(),
            call_id: None,
        },
        name: name.to_owned().try_into().unwrap(),
        input: input.as_object().unwrap().clone(),
    }
}

fn question(required: bool) -> ContentBlock {
    tool(
        "request_user_input",
        "ask",
        json!({
            "id":"target", "prompt":"Which target?", "required":required,
            "choices":[{"id":"lib","label":"Library"},{"id":"bin","label":"Binary"}],
            "allow_free_text":true
        }),
    )
}

async fn command(h: &Harness, command: Command) -> String {
    h.actor
        .send_message(Message::Command(command.clone()))
        .unwrap();
    let packet = h.event(|packet| matches!(packet, ActorToTuiPacket::CommandResult(found, _) if found == &command)).await;
    match packet {
        ActorToTuiPacket::CommandResult(_, text) => text,
        _ => panic!("Expected command result"),
    }
}

fn step(id: &str) -> PlanStep {
    PlanStep {
        id: id.into(),
        description: format!("Inspect {id}"),
        dependencies: vec![],
        acceptance: "Relevant source was inspected".into(),
        state: StepState::Pending,
        evidence: vec![],
        blocked_reason: None,
    }
}

#[test]
fn structured_questions_reject_invalid_ids_choices_answers_and_extra_permissions() {
    use common_models::interaction::{Answer, Question};
    let input = json!({"id":"target","prompt":"Which target?","required":true,"choices":[{"id":"lib","label":"Library"}],"allow_free_text":false});
    let question: Question = serde_json::from_value(input.clone()).unwrap();
    assert!(question.answer(&Answer::Text("Library".into())).is_err());
    assert!(
        question
            .answer(&Answer::Choice {
                choice_id: "missing".into()
            })
            .is_err()
    );
    assert_eq!(
        question
            .answer(&Answer::Choice {
                choice_id: "lib".into()
            })
            .unwrap(),
        "Library"
    );
    for invalid in [
        json!({"id":"../target"}),
        json!({"prompt":" "}),
        json!({"choices":[]}),
        json!({"choices":[{"id":"lib","label":"one"},{"id":"lib","label":"two"}]}),
        json!({"workspace":"/"}),
    ] {
        let mut value = input.clone();
        value
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert!(serde_json::from_value::<Question>(value).is_err());
    }
    let legacy: Question =
        serde_json::from_value(json!({"id":"old","prompt":"Old saved question?","required":false}))
            .unwrap();
    assert!(legacy.allow_free_text);
    assert!(legacy.choices.is_empty());
}

#[tokio::test]
async fn a_new_prompt_cannot_implicitly_answer_a_required_question() {
    let h = Harness::new(vec![], Duration::from_secs(2)).await;
    h.start("Investigate");
    answer(h.request().await.1, response(vec![question(true)]));
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::WaitingForInput,
                ..
            }
        )
    })
    .await;
    h.start("Binary");
    h.event(|packet| matches!(packet, ActorToTuiPacket::Queued { .. }))
        .await;
    assert!(h.requests.is_empty());
    assert!(command(&h, Command::Questions).await.contains("required"));
    command(&h, Command::parse("answer target choice lib").unwrap()).await;
    let (request, reply) = h.request().await;
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text() == "Binary")
    );
    assert!(request.messages.iter().any(|message| {
        message.text().contains("Answer to question target") && message.text().contains("Library")
    }));
    answer(reply, response(vec![text("Done")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[test]
fn plan_transitions_require_dependencies_real_evidence_and_reconciliation() {
    use common_models::interaction::PlanEvidence;
    let evidence = [("tool:read".into(), "read_file".into())].into();
    let mut input = PlanUpdate {
        revision: 0,
        requirements_revision: 0,
        steps: vec![step("inspect"), step("implement")],
    };
    input.steps[1].dependencies.push("inspect".into());
    let plan = Plan::default().update(input.clone(), 0, &evidence).unwrap();
    assert!(plan.update(input.clone(), 0, &evidence).is_err());
    input.revision = 1;
    input.steps[1].state = StepState::InProgress;
    assert!(plan.update(input.clone(), 0, &evidence).is_err());
    input.steps[1].state = StepState::Pending;
    input.steps[0].state = StepState::Completed;
    assert!(plan.update(input.clone(), 0, &evidence).is_err());
    input.steps[0].state = StepState::InProgress;
    let plan = plan.update(input.clone(), 0, &evidence).unwrap();
    input.revision = 2;
    input.steps[0].state = StepState::Completed;
    input.steps[0].evidence = vec![PlanEvidence {
        source: "tool:invented".into(),
        explanation: "Read the source".into(),
    }];
    assert!(plan.update(input.clone(), 0, &evidence).is_err());
    input.steps[0].evidence[0].source = "tool:read".into();
    input.steps[1].state = StepState::InProgress;
    let plan = plan.update(input.clone(), 0, &evidence).unwrap();
    input.revision = 3;
    input.requirements_revision = 1;
    assert!(plan.update(input.clone(), 1, &evidence).is_err());
    input.steps[0].state = StepState::Pending;
    input.steps[1].state = StepState::Pending;
    assert!(plan.update(input.clone(), 1, &evidence).is_ok());
    input.steps[0].dependencies = vec!["implement".into()];
    assert!(plan.update(input.clone(), 1, &evidence).is_err());
    input.steps[0].dependencies.clear();
    input.steps[0].state = StepState::Blocked;
    assert!(plan.update(input.clone(), 1, &evidence).is_err());
    input.steps[0].blocked_reason = Some("Waiting for target selection".into());
    assert!(plan.update(input, 1, &evidence).is_ok());
}

#[tokio::test]
async fn required_question_pauses_and_prevents_later_batch_writes_until_typed_answer() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let h = Harness::new(vec![write], Duration::from_secs(2)).await;
    h.start("Implement the selected target");
    answer(
        h.request().await.1,
        response(vec![question(true), call("write", "denied")]),
    );
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::WaitingForInput,
                ..
            }
        )
    })
    .await;
    assert!(h.requests.is_empty());
    assert!(entered.is_empty());
    assert!(
        command(&h, Command::parse("answer target choice missing").unwrap())
            .await
            .contains("Unknown choice")
    );
    assert!(h.requests.is_empty());
    command(&h, Command::parse("answer target choice lib").unwrap()).await;
    let (request, reply) = h.request().await;
    let messages = serde_json::to_string(&request.messages).unwrap();
    assert!(messages.contains("Library"));
    assert!(messages.contains("Not executed"));
    assert!(!messages.contains("Pending user questions (unanswered)"));
    answer(reply, response(vec![call("write", "allowed")]));
    let (_, release) = within(entered.recv_async()).await.unwrap();
    release.send(()).unwrap();
    answer(h.request().await.1, response(vec![text("Finished")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn answers_during_tools_preserve_complete_exchanges_and_durable_order() {
    for required in [false, true] {
        let workspace = crate::session::tests::Workspace::new();
        let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
        let store = runtime.sessions.clone().unwrap();
        let (read, entered) = gate("read", ToolEffect::Read);
        let h = Harness::with_runtime(vec![read], runtime).await;
        h.start("Investigate the target");
        answer(
            h.request().await.1,
            response(vec![call("read", "inspect"), question(required)]),
        );
        let (_, release) = within(entered.recv_async()).await.unwrap();
        h.event(|packet| matches!(packet, ActorToTuiPacket::InteractionUpdated(view) if view.questions.len() == 1)).await;
        command(
            &h,
            Command::parse("answer target text Use the library; keep APIs stable").unwrap(),
        )
        .await;
        let snapshot = store.list().unwrap().into_iter().next().unwrap();
        assert!(snapshot.questions.is_empty());
        assert_eq!(snapshot.deferred_input.len(), 1);
        assert!(snapshot.pending.is_some());
        release.send(()).unwrap();
        let (request, reply) = h.request().await;
        let messages = &request.messages;
        let answer_index = messages
            .iter()
            .position(|message| message.text().starts_with("Answer to question target"))
            .unwrap();
        assert!(messages[..answer_index].iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        }));
        assert_eq!(
            messages
                .iter()
                .filter(|message| message.text().starts_with("Answer to question target"))
                .count(),
            1
        );
        answer(reply, response(vec![text("Inspection complete")]));
        h.terminal(Lifecycle::Completed).await;
        let snapshot = store.list().unwrap().into_iter().next().unwrap();
        assert!(snapshot.deferred_input.is_empty());
        assert_eq!(
            serde_json::to_value(snapshot.history).unwrap(),
            serde_json::to_value(h.history().await).unwrap()
        );
        h.stop().await;
    }
}

#[tokio::test]
async fn optional_questions_allow_continuation_and_persist_without_inferred_answers() {
    let h = Harness::new(vec![], Duration::from_secs(2)).await;
    h.start("Investigate");
    answer(h.request().await.1, response(vec![question(false)]));
    let (request, reply) = h.request().await;
    assert!(
        serde_json::to_string(&request.messages)
            .unwrap()
            .contains("Pending user questions (unanswered)")
    );
    answer(
        reply,
        response(vec![text("Independent investigation finished")]),
    );
    h.terminal(Lifecycle::Completed).await;
    assert!(command(&h, Command::Questions).await.contains("optional"));
    command(&h, Command::parse("answer target choice bin").unwrap()).await;
    assert_eq!(
        command(&h, Command::Questions).await,
        "No pending questions."
    );
    assert!(
        command(&h, Command::parse("answer target choice bin").unwrap())
            .await
            .contains("not pending")
    );
    h.stop().await;
}

#[tokio::test]
async fn resume_pending_questions_restores_mode_and_clear_and_new_reset_interaction() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let h = Harness::with_runtime(vec![], runtime).await;
    command(&h, Command::Plan).await;
    h.start("Plan the target");
    answer(h.request().await.1, response(vec![question(true)]));
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::WaitingForInput,
                ..
            }
        )
    })
    .await;
    let id = store.list().unwrap()[0].id.clone();
    h.stop().await;
    let h = Harness::with_runtime(
        vec![],
        Runtime {
            sessions: Some(store.clone()),
            scope: utils::execution::ExecutionScope::with_workspace(
                utils::workspace::WorkspacePolicy::workspace(workspace.path.clone()).unwrap(),
            ),
            ..Runtime::default()
        },
    )
    .await;
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.clone(),
        })))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(Ok(_))))
        .await;
    assert!(h.requests.is_empty());
    assert!(command(&h, Command::Questions).await.contains("required"));
    command(&h, Command::parse("answer target choice lib").unwrap()).await;
    let (request, reply) = h.request().await;
    assert!(request.system.unwrap().contains("Work mode: Plan"));
    answer(reply, response(vec![text("Plan prepared")]));
    h.terminal(Lifecycle::Completed).await;
    for reset in [Command::Clear, Command::New] {
        command(&h, reset).await;
        assert_eq!(
            command(&h, Command::Questions).await,
            "No pending questions."
        );
        h.start("Plan again");
        let (request, reply) = h.request().await;
        assert!(request.system.unwrap().contains("Work mode: Implement"));
        answer(reply, response(vec![question(false)]));
        answer(h.request().await.1, response(vec![text("Done")]));
        h.terminal(Lifecycle::Completed).await;
        command(&h, Command::Plan).await;
    }
    assert!(
        store
            .list()
            .unwrap()
            .iter()
            .any(|snapshot| snapshot.id == id
                && snapshot.planning.mode == common_models::interaction::WorkMode::Plan)
    );
    h.stop().await;
}

#[tokio::test]
async fn steering_cancels_active_tools_and_queue_then_reconciles_plan() {
    let (write, entered) = gate("write", ToolEffect::Write);
    let active = write.active.clone();
    let h = Harness::new(vec![write], Duration::from_secs(2)).await;
    h.start("Implement the binary");
    answer(
        h.request().await.1,
        response(vec![tool(
            "update_plan",
            "plan",
            serde_json::to_value(PlanUpdate {
                revision: 0,
                requirements_revision: 0,
                steps: vec![step("inspect")],
            })
            .unwrap(),
        )]),
    );
    answer(h.request().await.1, response(vec![call("write", "old")]));
    let (_, release) = within(entered.recv_async()).await.unwrap();
    h.start("Obsolete follow-up");
    h.event(|packet| matches!(packet, ActorToTuiPacket::Queued { .. }))
        .await;
    command(
        &h,
        Command::Steer("Use the library instead and preserve the public API".into()),
    )
    .await;
    h.terminal(Lifecycle::Cancelled).await;
    let (request, reply) = h.request().await;
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(release.send(()).is_err());
    let sent = serde_json::to_string(&request.messages).unwrap();
    assert!(sent.contains("Use the library instead"));
    assert!(!sent.contains("Obsolete follow-up"));
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains("\"requirements_revision\":1"))
    );
    answer(reply, response(vec![call("write", "stale")]));
    let (request, reply) = h.request().await;
    assert!(
        serde_json::to_string(&request.messages)
            .unwrap()
            .contains("reconcile the current plan")
    );
    assert!(entered.is_empty());
    answer(
        reply,
        response(vec![tool(
            "update_plan",
            "revised",
            serde_json::to_value(PlanUpdate {
                revision: 1,
                requirements_revision: 1,
                steps: vec![step("inspect")],
            })
            .unwrap(),
        )]),
    );
    let (_, reply) = h.request().await;
    answer(reply, response(vec![text("Revised task prepared")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn tracked_plan_uses_observed_evidence_and_survives_compaction_and_fork() {
    use common_models::interaction::PlanEvidence;
    let workspace = crate::session::tests::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolEffect::Read);
    let h = Harness::with_runtime(vec![read], runtime).await;
    h.start("Inspect and document the target");
    let mut update = PlanUpdate {
        revision: 0,
        requirements_revision: 0,
        steps: vec![step("inspect")],
    };
    answer(
        h.request().await.1,
        response(vec![tool(
            "update_plan",
            "plan",
            serde_json::to_value(&update).unwrap(),
        )]),
    );
    answer(
        h.request().await.1,
        response(vec![call("read", "evidence")]),
    );
    let (_, release) = within(entered.recv_async()).await.unwrap();
    release.send(()).unwrap();
    let (request, reply) = h.request().await;
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains("tool:evidence"))
    );
    update.revision = 1;
    update.steps[0].state = StepState::InProgress;
    answer(
        reply,
        response(vec![tool(
            "update_plan",
            "started",
            serde_json::to_value(&update).unwrap(),
        )]),
    );
    let (_, reply) = h.request().await;
    update.revision = 2;
    update.steps[0].state = StepState::Completed;
    update.steps[0].evidence = vec![PlanEvidence {
        source: "tool:evidence".into(),
        explanation: "Inspected the selected source".into(),
    }];
    answer(
        reply,
        response(vec![tool(
            "update_plan",
            "finished",
            serde_json::to_value(&update).unwrap(),
        )]),
    );
    answer(h.request().await.1, response(vec![question(false)]));
    answer(
        h.request().await.1,
        response(vec![text("Inspection documented")]),
    );
    h.terminal(Lifecycle::Completed).await;
    let saved = store.list().unwrap().into_iter().next().unwrap();
    assert_eq!(saved.planning.plan.revision, 3);
    assert_eq!(saved.planning.plan.steps[0].state, StepState::Completed);
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (request, reply) = h.request().await;
    assert!(request.tools.is_empty());
    answer(
        reply,
        response(vec![text(
            "Source inspected and documented; optional target question remains pending",
        )]),
    );
    h.terminal(Lifecycle::Completed).await;
    command(&h, Command::Fork).await;
    let snapshots = store.list().unwrap();
    assert_eq!(snapshots.len(), 2);
    for snapshot in snapshots {
        assert_eq!(snapshot.planning.plan, saved.planning.plan);
        assert_eq!(snapshot.questions.len(), 1);
        assert_eq!(snapshot.context.generation, 1);
    }
    h.stop().await;
}

#[tokio::test]
async fn questions_do_not_wait_for_an_independent_workspace_writer() {
    let runtime = Runtime::default();
    let lease = runtime
        .workspace
        .acquire(ToolEffect::Write, &runtime.scope)
        .await
        .unwrap();
    let h = Harness::with_runtime(vec![], runtime.clone()).await;
    h.start("Investigate while validation is running");
    answer(h.request().await.1, response(vec![question(false)]));
    let (request, reply) = h.request().await;
    assert!(
        serde_json::to_string(&request.messages)
            .unwrap()
            .contains("Pending user questions (unanswered)")
    );
    drop(lease);
    answer(
        reply,
        response(vec![text("Investigation continues independently")]),
    );
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn clear_and_new_cancel_waiting_queues_and_archive_unanswered_questions() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let h = Harness::with_runtime(vec![], runtime).await;
    for reset in [Command::Clear, Command::New] {
        h.start("Plan the target");
        answer(h.request().await.1, response(vec![question(true)]));
        h.event(|packet| {
            matches!(
                packet,
                ActorToTuiPacket::TurnChanged {
                    state: Lifecycle::WaitingForInput,
                    ..
                }
            )
        })
        .await;
        h.start("Queued work");
        h.event(|packet| matches!(packet, ActorToTuiPacket::Queued { .. }))
            .await;
        command(&h, reset).await;
        assert_eq!(
            command(&h, Command::Questions).await,
            "No pending questions."
        );
        assert!(h.requests.is_empty());
        assert_eq!(h.history().await.len(), 1);
    }
    let archived = store
        .list()
        .unwrap()
        .into_iter()
        .filter(|snapshot| !snapshot.questions.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(archived.len(), 2);
    for snapshot in archived {
        assert!(snapshot.queued.is_empty());
        assert!(snapshot.questions[0].required);
        assert_eq!(snapshot.status, Lifecycle::Cancelled);
    }
    h.stop().await;
}
