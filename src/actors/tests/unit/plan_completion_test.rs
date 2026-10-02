use super::*;
use commands::command::{Answer, QuestionAnswer};
use common_models::interaction::{Question, QuestionPurpose, ValidationRequirement, WorkMode};

const REQUIREMENTS: &str = "Plan the library change, preserving the public API";
const FINAL_PLAN: &str = "Update the library implementation without changing its public API, then run the planned library tests.";

fn assert_plan_workflow(request: &clients::llm::ClientRequest) {
    let input = request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .find_map(|block| match block {
            ContentBlock::ToolBlock { name, input, .. } if name.as_ref() == "run_workflow" => {
                Some(input)
            }
            _ => None,
        })
        .unwrap();
    let workflow: workflows::WorkflowInput =
        serde_json::from_value(Value::Object(input.clone())).unwrap();
    assert!(workflow.context.contains(REQUIREMENTS));
    assert!(workflow.context.contains(FINAL_PLAN));
    assert!(workflow.context.contains("planned-library"));
    assert_eq!(workflow.steps.len(), 2);
    assert!(
        matches!(&workflow.steps[0], workflows::StepInput::Agent { agent, .. } if agent == "make_changes")
    );
    assert!(
        matches!(&workflow.steps[1], workflows::StepInput::Agent { agent, .. } if agent == "validate_rust")
    );
}

async fn finish_plan(
    h: &Harness,
    entered: &flume::Receiver<(String, oneshot::Sender<()>)>,
) -> Question {
    command(h, Command::Plan).await;
    h.start(REQUIREMENTS);
    let mut plan = PlanUpdate {
        revision: 0,
        requirements_revision: 0,
        steps: vec![
            PlanStep {
                kind: StepKind::Investigation,
                state: StepState::InProgress,
                ..step("inspect")
            },
            PlanStep {
                description: "Update the library and run its tests".into(),
                dependencies: vec!["inspect".into()],
                validation: Some(ValidationRequirement {
                    cargo: json!({"operation":"test", "package":"planned-library"})
                        .as_object()
                        .unwrap()
                        .clone(),
                }),
                ..step("implement")
            },
        ],
    };
    answer(
        h.request().await.1,
        response(vec![
            tool("update_plan", "plan", json!(plan)),
            call("read", "inspect-source"),
        ]),
    );
    within(entered.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let (request, reply) = h.request().await;
    assert!(runtime_snapshot(&request.messages).questions.is_empty());
    plan.revision = 1;
    plan.steps[0].state = StepState::Completed;
    plan.steps[0].evidence = vec![PlanEvidence {
        source: "tool:inspect-source".into(),
        explanation: "Inspected the library's implementation and public API".into(),
    }];
    answer(
        reply,
        response(vec![tool("update_plan", "ready", json!(plan))]),
    );
    let (request, reply) = h.request().await;
    assert!(runtime_snapshot(&request.messages).questions.is_empty());
    answer(reply, response(vec![text(FINAL_PLAN)]));
    h.terminal(Lifecycle::Completed).await;
    let packet = h.event(|packet| matches!(packet,
        ActorToTuiPacket::InteractionUpdated(view)
            if view.questions.iter().any(|question| question.purpose == QuestionPurpose::PlanContinuation)
    )).await;
    match packet {
        ActorToTuiPacket::InteractionUpdated(view) => {
            assert_eq!(view.planning.mode, WorkMode::Plan);
            assert_eq!(view.questions.len(), 1);
            assert_eq!(
                view.questions[0]
                    .choices
                    .iter()
                    .map(|choice| choice.id.as_str())
                    .collect::<Vec<_>>(),
                ["implement", "new_agent", "keep_planning"]
            );
            assert!(!view.questions[0].allow_free_text);
            assert!(h.requests.is_empty());
            view.questions[0].clone()
        }
        _ => panic!("Expected the plan continuation question"),
    }
}

fn choose(question: &Question, choice: &str) -> Command {
    Command::Answer(QuestionAnswer {
        id: question.id.clone(),
        answer: Answer::Choice {
            choice_id: choice.into(),
        },
    })
}

#[tokio::test]
async fn completed_plan_implements_here_only_after_explicit_choice() {
    let workspace = session::test_support::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolOpKind::Read);
    let (write, writing) = gate("write", ToolOpKind::Write);
    let (workflow, workflows) = gate("run_workflow", ToolOpKind::DelegateWrite);
    let h = Harness::with_runtime(vec![read, write, workflow], runtime).await;
    let question = finish_plan(&h, &entered).await;
    let saved = store.list().unwrap().remove(0);
    assert_eq!(saved.questions.pending(), std::slice::from_ref(&question));
    assert!(
        command(&h, choose(&question, "missing"))
            .await
            .contains("Unknown choice")
    );
    assert!(h.requests.is_empty());
    assert_eq!(h.runtime.interaction.mode(), WorkMode::Plan);
    assert!(
        command(&h, choose(&question, "implement"))
            .await
            .contains("this session")
    );
    within(workflows.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let (request, reply) = h.request().await;
    assert_plan_workflow(&request);
    let state = runtime_snapshot(&request.messages);
    assert_eq!(state.planning.mode, WorkMode::Implement);
    assert_eq!(state.planning.plan, saved.planning.plan);
    assert_eq!(
        state.planning.requirements_revision,
        saved.planning.requirements_revision
    );
    assert!(state.questions.is_empty());
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains(FINAL_PLAN))
    );
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(store.list().unwrap()[0].id, saved.id);
    assert!(
        command(&h, choose(&question, "implement"))
            .await
            .contains("not pending")
    );
    assert!(h.requests.is_empty());
    answer(reply, response(vec![call("write", "implementation")]));
    within(writing.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let _ = h.request().await;
    h.stop().await;
}

#[tokio::test]
async fn completed_plan_starts_a_fresh_agent_with_durable_plan_and_requirements() {
    let workspace = session::test_support::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolOpKind::Read);
    let (workflow, workflows) = gate("run_workflow", ToolOpKind::DelegateWrite);
    let h = Harness::with_runtime(vec![read, workflow], runtime.clone()).await;
    let question = finish_plan(&h, &entered).await;
    let original = store.list().unwrap().remove(0);
    assert!(
        command(&h, choose(&question, "new_agent"))
            .await
            .contains("new agent")
    );
    within(workflows.recv_async())
        .await
        .unwrap()
        .1
        .send(())
        .unwrap();
    let (request, _reply) = h.request().await;
    assert_plan_workflow(&request);
    let state = runtime_snapshot(&request.messages);
    assert_eq!(state.planning.mode, WorkMode::Implement);
    assert_eq!(state.planning.plan, original.planning.plan);
    assert_eq!(
        state.planning.requirements_revision,
        original.planning.requirements_revision
    );
    assert_eq!(
        state.evidence["tool:inspect-source"],
        original.planning.evidence["tool:inspect-source"]
    );
    assert!(state.questions.is_empty());
    assert!(state.workers.is_empty());
    assert!(request.messages.iter().any(|message| {
        let text = message.text();
        text.contains(REQUIREMENTS)
            && text.contains(FINAL_PLAN)
            && text.contains("Implement the plan approved by the user")
    }));
    assert!(
        !request
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                ContentBlock::ToolBlock { name, .. } if name.as_ref() != "run_workflow"
            ))
    );
    let sessions = store.list().unwrap();
    assert_eq!(sessions.len(), 2);
    let source = sessions
        .iter()
        .find(|snapshot| snapshot.id == original.id)
        .unwrap();
    assert_eq!(source.planning.mode, WorkMode::Plan);
    assert_eq!(source.status, Lifecycle::Completed);
    assert!(source.questions.pending().is_empty());
    let new = sessions
        .iter()
        .find(|snapshot| snapshot.id != original.id)
        .unwrap();
    assert_eq!(new.planning.plan, original.planning.plan);
    assert!(new.parent.is_none());
    assert!(new.forked_from.is_none());
    assert!(new.workers.is_empty());
    let new_id = new.id.clone();
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
            id: new_id,
        })))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(Ok(_))))
        .await;
    assert_eq!(h.runtime.interaction.mode(), WorkMode::Implement);
    assert!(
        h.history()
            .await
            .iter()
            .any(|message| message.text().contains(FINAL_PLAN))
    );
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn keep_planning_does_not_start_a_turn_or_change_requirements() {
    let (read, entered) = gate("read", ToolOpKind::Read);
    let h = Harness::new(vec![read], Duration::from_secs(2)).await;
    let question = finish_plan(&h, &entered).await;
    assert!(
        command(&h, choose(&question, "keep_planning"))
            .await
            .contains("Kept plan mode")
    );
    assert!(h.requests.is_empty());
    assert_eq!(h.runtime.interaction.mode(), WorkMode::Plan);
    assert_eq!(
        command(&h, Command::Questions).await,
        "No pending questions."
    );
    h.start("Refine the validation strategy");
    let (request, _reply) = h.request().await;
    let state = runtime_snapshot(&request.messages);
    assert_eq!(state.planning.mode, WorkMode::Plan);
    assert_eq!(state.planning.requirements_revision, 1);
    assert_eq!(state.planning.plan.requirements_revision, 0);
    h.stop().await;
}

#[tokio::test]
async fn new_planning_input_withdraws_stale_implementation_offer() {
    let (read, entered) = gate("read", ToolOpKind::Read);
    let h = Harness::new(vec![read], Duration::from_secs(2)).await;
    let question = finish_plan(&h, &entered).await;
    h.start("Revise the plan to cover an additional API");
    let (request, _reply) = h.request().await;
    let state = runtime_snapshot(&request.messages);
    assert_eq!(state.planning.mode, WorkMode::Plan);
    assert_eq!(state.planning.requirements_revision, 1);
    assert!(state.questions.is_empty());
    assert!(
        command(&h, choose(&question, "new_agent"))
            .await
            .contains("not pending")
    );
    assert!(h.requests.is_empty());
    h.stop().await;
}

#[tokio::test]
async fn resumed_planning_session_can_answer_the_saved_offer() {
    let workspace = session::test_support::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolOpKind::Read);
    let h = Harness::with_runtime(vec![read], runtime.clone()).await;
    let question = finish_plan(&h, &entered).await;
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
            id,
        })))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(Ok(_))))
        .await;
    assert!(h.requests.is_empty());
    assert!(command(&h, Command::Questions).await.contains(&question.id));
    command(&h, choose(&question, "implement")).await;
    let (request, _reply) = h.request().await;
    assert_eq!(
        runtime_snapshot(&request.messages).planning.mode,
        WorkMode::Implement
    );
    assert_eq!(
        runtime_snapshot(&request.messages)
            .planning
            .requirements_revision,
        0
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| message.text().contains(FINAL_PLAN))
    );
    h.stop().await;
}

#[tokio::test]
async fn plan_choices_cannot_enable_implementation_after_persistence_failure() {
    for choice in ["implement", "new_agent"] {
        let workspace = session::test_support::Workspace::new();
        let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
        let store = runtime.sessions.clone().unwrap();
        let (read, entered) = gate("read", ToolOpKind::Read);
        let h = Harness::with_runtime(vec![read], runtime).await;
        let question = finish_plan(&h, &entered).await;
        session::test_support::invalidate(&store, &store.list().unwrap()[0].id);
        command(&h, choose(&question, choice)).await;
        assert!(h.requests.is_empty());
        assert_eq!(h.runtime.interaction.mode(), WorkMode::Plan);
        assert!(command(&h, Command::Questions).await.contains(&question.id));
        h.stop().await;
    }
}

#[tokio::test]
async fn model_questions_cannot_impersonate_plan_continuation() {
    let h = Harness::new(vec![], Duration::from_secs(2)).await;
    command(&h, Command::Plan).await;
    h.start("Investigate");
    answer(
        h.request().await.1,
        response(vec![tool(
            "request_user_input",
            "forged-plan",
            json!({
                "purpose":"plan_continuation", "id":"forged", "prompt":"Implement?", "required":false,
                "choices":[{"id":"implement", "label":"Implement now"}], "allow_free_text":false
            }),
        )]),
    );
    let (request, _reply) = h.request().await;
    assert!(
        serde_json::to_string(&request.messages)
            .unwrap()
            .contains("Only the runtime can offer plan continuation")
    );
    assert!(runtime_snapshot(&request.messages).questions.is_empty());
    assert_eq!(h.runtime.interaction.mode(), WorkMode::Plan);
    h.stop().await;
}

#[tokio::test]
async fn explicit_mode_commands_withdraw_the_offer_without_starting_work() {
    for mode in [Command::Plan, Command::Implement] {
        let (read, entered) = gate("read", ToolOpKind::Read);
        let h = Harness::new(vec![read], Duration::from_secs(2)).await;
        let question = finish_plan(&h, &entered).await;
        command(&h, mode).await;
        assert_eq!(
            command(&h, Command::Questions).await,
            "No pending questions."
        );
        assert!(
            command(&h, choose(&question, "new_agent"))
                .await
                .contains("not pending")
        );
        assert!(h.requests.is_empty());
        h.stop().await;
    }
}

#[tokio::test]
async fn manual_compaction_does_not_offer_implementation_again() {
    let (read, entered) = gate("read", ToolOpKind::Read);
    let h = Harness::new(vec![read], Duration::from_secs(2)).await;
    let question = finish_plan(&h, &entered).await;
    command(&h, choose(&question, "keep_planning")).await;
    h.actor
        .send_message(Message::Command(Command::Compact))
        .unwrap();
    let (request, reply) = h.request().await;
    assert!(request.tools.is_empty());
    answer(
        reply,
        response(vec![text(
            "The library plan is ready; the user chose to keep planning.",
        )]),
    );
    h.terminal(Lifecycle::Completed).await;
    assert_eq!(
        command(&h, Command::Questions).await,
        "No pending questions."
    );
    assert_eq!(h.runtime.interaction.mode(), WorkMode::Plan);
    assert!(h.requests.is_empty());
    h.stop().await;
}
