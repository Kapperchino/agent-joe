use super::*;
use crate::{context::RequestMode, states::turn_machine::SessionEvent};
use clients::failure::{Failure, FailureKind};
use common_models::interaction::{Planning, WorkMode};

fn machine() -> TurnMachine {
    TurnMachine::new(Default::default(), RequestMode::Continue)
}

fn awaiting() -> MergeApproval {
    MergeApproval::Awaiting {
        question: "merge-fixture".into(),
        commit: "approved-commit".into(),
    }
}

fn resolving(turn: TurnId) -> MergeApproval {
    MergeApproval::None
        .transition(MergeEvent::Conflicted {
            conflict: MergeConflict {
                approved: "approved-commit".into(),
                target: "main-commit".into(),
                paths: vec!["lib.rs".into()],
            },
            turn,
        })
        .unwrap()
}

fn failed_storage() -> Persistence {
    Persistence::Failed(Failure::new(FailureKind::Tool, "Fixture storage failure"))
}

#[test]
fn starting_a_task_revokes_pending_approval() {
    assert!(matches!(
        awaiting().transition(MergeEvent::TaskStarted {
            turn: TurnId::new(),
        }),
        Some(MergeApproval::None)
    ));
    assert!(
        MergeApproval::None
            .transition(MergeEvent::TaskStarted {
                turn: TurnId::new(),
            })
            .is_none()
    );
}

#[test]
fn only_the_approved_resolution_turn_keeps_running() {
    let turn = TurnId::new();
    let approval = resolving(turn);
    assert!(approval.resolution(turn).is_some());
    assert!(approval.question().is_none());
    assert!(
        approval
            .transition(MergeEvent::TaskStarted { turn })
            .is_none()
    );
    let unrelated = TurnId::new();
    assert!(approval.resolution(unrelated).is_none());
    let paused = approval
        .transition(MergeEvent::TaskStarted { turn: unrelated })
        .unwrap();
    assert!(matches!(
        &paused,
        MergeApproval::Resolving {
            conflict,
            activity: ResolutionActivity::Paused,
        } if conflict.approved == "approved-commit" && conflict.target == "main-commit"
    ));
    assert!(paused.resolution(turn).is_none());
    assert!(
        paused
            .transition(MergeEvent::TaskStarted { turn })
            .is_none()
    );
}

#[test]
fn pausing_changes_only_running_resolutions() {
    let turn = TurnId::new();
    let paused = resolving(turn).transition(MergeEvent::Paused).unwrap();
    assert!(paused.resolution(turn).is_none());
    for approval in [MergeApproval::None, awaiting(), paused] {
        assert!(approval.transition(MergeEvent::Paused).is_none());
    }
}

#[test]
fn proposals_preserve_the_new_commit_before_automatic_merge() {
    let turn = TurnId::new();
    let approval = resolving(turn);
    let proposal = MergeProposal::new(&approval, turn, Some("resolved-commit".into()));
    assert!(matches!(
        &proposal,
        MergeProposal::Approved { commit } if commit == "resolved-commit"
    ));
    let pending = approval.transition(proposal.event()).unwrap();
    assert!(matches!(
        &pending,
        MergeApproval::Awaiting { commit, .. } if commit == "resolved-commit"
    ));
    let question = pending.question().unwrap();
    assert!(question.id.starts_with("merge-"));
    assert!(question.prompt.contains("resolved-commit"));
    assert!(!question.required);
    assert!(!question.allow_free_text);
}

#[test]
fn only_an_active_matching_resolution_can_skip_approval() {
    let turn = TurnId::new();
    let paused = resolving(turn).transition(MergeEvent::Paused).unwrap();
    for approval in [
        MergeApproval::None,
        awaiting(),
        resolving(TurnId::new()),
        paused,
    ] {
        let proposal = MergeProposal::new(&approval, turn, Some("new-commit".into()));
        assert!(matches!(
            &proposal,
            MergeProposal::AwaitingApproval { commit } if commit == "new-commit"
        ));
        assert!(matches!(
            proposal.event(),
            MergeEvent::Proposed { commit } if commit == "new-commit"
        ));
    }
}

#[test]
fn an_empty_proposal_finishes_every_approval_state() {
    let turn = TurnId::new();
    let paused = resolving(turn).transition(MergeEvent::Paused).unwrap();
    for approval in [MergeApproval::None, awaiting(), resolving(turn), paused] {
        let proposal = MergeProposal::new(&approval, turn, None);
        assert!(matches!(&proposal, MergeProposal::Empty));
        assert!(matches!(proposal.event(), MergeEvent::Finished));
        assert!(matches!(
            approval.transition(proposal.event()),
            Some(MergeApproval::None)
        ));
    }
}

#[test]
fn persisted_approvals_keep_their_format_and_restore_resolutions_paused() {
    let saved = serde_json::to_value(awaiting()).unwrap();
    assert_eq!(
        saved,
        serde_json::json!({
            "Awaiting": {
                "question": "merge-fixture",
                "commit": "approved-commit"
            }
        })
    );
    let restored: MergeApproval = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.question(), awaiting().question());

    let turn = TurnId::new();
    let saved = serde_json::to_value(resolving(turn)).unwrap();
    assert_eq!(
        saved,
        serde_json::json!({
            "Resolving": {
                "conflict": {
                    "approved": "approved-commit",
                    "target": "main-commit",
                    "paths": ["lib.rs"]
                }
            }
        })
    );
    let restored: MergeApproval = serde_json::from_value(saved).unwrap();
    assert!(matches!(
        &restored,
        MergeApproval::Resolving {
            activity: ResolutionActivity::Paused,
            ..
        }
    ));
    assert!(matches!(
        MergeProposal::new(&restored, turn, Some("resolved-commit".into())),
        MergeProposal::AwaitingApproval { .. }
    ));
}

#[test]
fn merge_decisions_require_a_pending_question_and_a_listed_choice() {
    let turn = machine();
    let merge = Answer::Choice {
        choice_id: "merge".into(),
    };
    assert!(matches!(
        MergeDecision::new(&awaiting(), &merge, &turn, &Persistence::Ready).unwrap(),
        MergeDecision::Merge { commit } if commit == "approved-commit"
    ));
    assert!(matches!(
        MergeDecision::new(
            &awaiting(),
            &Answer::Choice {
                choice_id: "keep".into()
            },
            &turn,
            &Persistence::Ready,
        )
        .unwrap(),
        MergeDecision::Keep
    ));
    for answer in [
        Answer::Choice {
            choice_id: "unknown".into(),
        },
        Answer::Text("merge".into()),
    ] {
        assert!(MergeDecision::new(&awaiting(), &answer, &turn, &Persistence::Ready).is_err());
    }
    for approval in [MergeApproval::None, resolving(TurnId::new())] {
        assert_eq!(
            MergeDecision::new(&approval, &merge, &turn, &Persistence::Ready)
                .err()
                .unwrap()
                .to_string(),
            "No merge is awaiting approval"
        );
    }
}

#[test]
fn active_tasks_and_failed_storage_block_both_merge_choices() {
    let idle = machine();
    let mut active = machine();
    active.transition(SessionEvent::Start(FollowUp::new(Some("task".into()))));
    assert!(!active.is_idle());
    for choice in ["merge", "keep"] {
        let answer = Answer::Choice {
            choice_id: choice.into(),
        };
        assert!(MergeDecision::new(&awaiting(), &answer, &active, &Persistence::Ready).is_err());
        assert!(MergeDecision::new(&awaiting(), &answer, &idle, &failed_storage()).is_err());
    }
}

#[test]
fn unavailable_offers_skip_snapshot_access_but_eligible_offers_propagate_errors() {
    let workspace = crate::session::tests::Workspace::new();
    let mut runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let session = store
        .create(clients::llm::SessionProvider::Injected, None, vec![])
        .unwrap();
    runtime.session = Some(session.clone());
    let idle = machine();
    assert!(
        MergeWorkspace::for_offer(&runtime, &idle, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
    crate::session::tests::invalidate(&store, &session.id);
    assert!(MergeWorkspace::for_offer(&runtime, &idle, &Persistence::Ready, false).is_err());
    assert!(
        MergeWorkspace::for_offer(&runtime, &idle, &Persistence::Ready, true)
            .unwrap()
            .is_none()
    );
    assert!(
        MergeWorkspace::for_offer(&runtime, &idle, &failed_storage(), false)
            .unwrap()
            .is_none()
    );

    let mut active = machine();
    active.transition(SessionEvent::Start(FollowUp::new(Some("task".into()))));
    assert!(
        MergeWorkspace::for_offer(&runtime, &active, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
    let helper = Runtime {
        role: ExecutionRole::Helper,
        ..runtime.clone()
    };
    assert!(
        MergeWorkspace::for_offer(&helper, &idle, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
    let unconfigured = Runtime {
        project: None,
        ..runtime.clone()
    };
    assert!(
        MergeWorkspace::for_offer(&unconfigured, &idle, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
    let inactive = Runtime {
        session: None,
        ..runtime.clone()
    };
    assert!(
        MergeWorkspace::for_offer(&inactive, &idle, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
    runtime.interaction.set(
        &Planning {
            mode: WorkMode::Plan,
            ..Default::default()
        },
        &Default::default(),
    );
    assert!(
        MergeWorkspace::for_offer(&runtime, &idle, &Persistence::Ready, false)
            .unwrap()
            .is_none()
    );
}
