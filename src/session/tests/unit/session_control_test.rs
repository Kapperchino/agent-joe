use crate::control::SessionControl;
use crate::persistence::{Persistence, SessionPersistence};
use crate::runtime::SessionRuntime;
use crate::{Event, test_support::Workspace};
use clients::llm::{Message, SessionProvider};
use commands::command::Command;
use common_models::interaction::WorkMode;
use common_models::runtime_ids::TurnId;
use common_models::tui_models::ActorToTuiPacket;
use conversation::Conversation;
use interaction::InteractionState;
use interaction::access::{InteractionReadiness, InteractionRole};
use interaction::control::InteractionControl;
use interaction::policy::InteractionPolicy;
use merge_workflow::execution::{MergeActivity, MergeEnvironment, SessionMerge};
use merge_workflow::{MergeApproval, MergeEvent};
use turn_engine::turn::FollowUp;

#[test]
fn interaction_publishes_only_committed_changes_without_an_actor() {
    let workspace = Workspace::new();
    let store = workspace.store();
    let session = store
        .create(SessionProvider::Injected, None, vec![])
        .unwrap();
    let mut state = InteractionState::default();
    let mut persistence = Persistence::Ready;
    let policy = InteractionPolicy::default();
    let (tui_tx, events) = flume::unbounded();
    let reporter = |packet| {
        tui_tx.send(packet).unwrap();
    };
    let mut control = InteractionControl {
        state: &mut state,
        persistence: SessionPersistence {
            state: &mut persistence,
            session: Some(&session),
            reporter: &reporter,
        },
        policy: &policy,
        role: InteractionRole::Root,
    };
    control
        .command(&Command::Plan, InteractionReadiness::Idle)
        .unwrap();
    assert_eq!(session.snapshot().unwrap().planning.mode, WorkMode::Plan);
    assert_eq!(policy.mode(), WorkMode::Plan);
    assert!(matches!(
        events.recv().unwrap(),
        ActorToTuiPacket::InteractionUpdated(_)
    ));

    crate::test_support::invalidate(&store, &session.id);
    for _ in 0..2 {
        assert!(
            control
                .command(&Command::Implement, InteractionReadiness::Idle)
                .is_err()
        );
    }
    assert_eq!(control.state.planning().mode, WorkMode::Plan);
    assert_eq!(policy.mode(), WorkMode::Plan);
    assert!(matches!(control.persistence.state, Persistence::Failed(_)));
    assert!(matches!(
        events.recv().unwrap(),
        ActorToTuiPacket::SessionError(_)
    ));
    assert!(events.is_empty());
}

#[test]
fn merge_recovery_and_new_tasks_keep_questions_and_storage_in_sync_without_an_actor() {
    let workspace = Workspace::new();
    let mut runtime = SessionRuntime::for_workspace(workspace.path.clone()).unwrap();
    let session = runtime
        .sessions
        .as_ref()
        .unwrap()
        .create(SessionProvider::Injected, None, vec![])
        .unwrap();
    let mut approval = MergeApproval::Approved {
        commit: "approved-commit".into(),
    };
    session
        .record(Event::MergeApproval(approval.clone()))
        .unwrap();
    runtime.session = Some(session.clone());
    let mut state = InteractionState::default();
    let mut persistence = Persistence::Ready;
    let (tui_tx, _events) = flume::unbounded();
    let reporter = |packet| {
        tui_tx.send(packet).unwrap();
    };
    let mut merge = SessionMerge {
        approval: &mut approval,
        interaction: InteractionControl {
            state: &mut state,
            persistence: SessionPersistence {
                state: &mut persistence,
                session: Some(&session),
                reporter: &reporter,
            },
            policy: &runtime.interaction,
            role: InteractionRole::Root,
        },
        environment: MergeEnvironment {
            project: runtime.project.as_ref(),
            workspace: &runtime.workspace,
            scope: &runtime.scope,
            request_timeout: std::time::Duration::from_secs(30),
        },
        activity: MergeActivity::Idle,
    };
    merge.restore_merge_question().unwrap();
    let snapshot = session.snapshot().unwrap();
    let question = snapshot.merge_approval.question().unwrap();
    assert_eq!(
        snapshot.questions.pending(),
        std::slice::from_ref(&question)
    );
    assert_eq!(
        merge.interaction.state.questions().pending(),
        std::slice::from_ref(&question)
    );

    merge
        .record_merge(MergeEvent::TaskStarted {
            turn: TurnId::new(),
        })
        .unwrap();
    let snapshot = session.snapshot().unwrap();
    assert!(matches!(snapshot.merge_approval, MergeApproval::None));
    assert!(snapshot.questions.pending().is_empty());
    assert!(merge.interaction.state.questions().pending().is_empty());

    crate::test_support::invalidate(runtime.sessions.as_ref().unwrap(), &session.id);
    assert!(
        merge
            .record_merge(MergeEvent::Proposed {
                commit: "next-commit".into()
            })
            .is_err()
    );
    assert!(matches!(merge.approval, MergeApproval::None));
    assert!(merge.interaction.state.questions().pending().is_empty());
}

#[test]
fn session_control_keeps_queued_input_and_live_history_in_sync_without_an_actor() {
    let workspace = Workspace::new();
    let history = vec![Message::new("Workspace context".into())];
    let session = workspace
        .store()
        .create(SessionProvider::Injected, None, history.clone())
        .unwrap();
    let mut conversation = Conversation::new(history, Some(session.id.clone()));
    let mut persistence = Persistence::Ready;
    let (tui_tx, _events) = flume::unbounded();
    let reporter = |packet| {
        tui_tx.send(packet).unwrap();
    };
    let mut control = SessionControl {
        conversation: &mut conversation,
        persistence: SessionPersistence {
            state: &mut persistence,
            session: Some(&session),
            reporter: &reporter,
        },
    };
    let input = FollowUp::new(Some("Inspect the source".into()));
    control.queue_input(&input);
    assert_eq!(session.snapshot().unwrap().queued.len(), 1);
    control.begin_turn(input);
    control.append_history(vec![Message::new_assistant("Source inspected".into())]);
    let snapshot = session.snapshot().unwrap();
    assert!(snapshot.queued.is_empty());
    assert_eq!(
        serde_json::to_value(snapshot.history).unwrap(),
        serde_json::to_value(control.conversation.history()).unwrap()
    );
}
