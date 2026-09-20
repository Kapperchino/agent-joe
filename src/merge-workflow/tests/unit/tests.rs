use super::*;

fn awaiting() -> MergeApproval {
    MergeApproval::Awaiting {
        question: "merge-fixture".into(),
        commit: "approved-commit".into(),
    }
}

fn approved() -> MergeApproval {
    MergeApproval::Approved {
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

#[test]
fn starting_a_task_revokes_pending_approval() {
    for approval in [awaiting(), approved()] {
        assert!(matches!(
            approval.transition(MergeEvent::TaskStarted {
                turn: TurnId::new(),
            }),
            Some(MergeApproval::None)
        ));
    }
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
        } if conflict.approved == "approved-commit"
    ));
    assert!(
        matches!(&paused, MergeApproval::Resolving { conflict, .. } if conflict.target == "main-commit")
    );
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
    for approval in [MergeApproval::None, awaiting(), approved(), paused] {
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
        MergeApproval::Approved { commit } if commit == "resolved-commit"
    ));
    assert!(pending.question().is_none());
    let question = awaiting().question().unwrap();
    assert!(question.id.starts_with("merge-"));
    assert!(question.prompt.contains("approved-commit"));
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
        approved(),
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
    for approval in [
        MergeApproval::None,
        awaiting(),
        approved(),
        resolving(turn),
        paused,
    ] {
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
    let merge = Answer::Choice {
        choice_id: "merge".into(),
    };
    assert!(matches!(
        MergeDecision::new(&awaiting(), "merge-fixture", &merge, MergeReadiness::Ready).unwrap(),
        MergeDecision::Merge { commit } if commit == "approved-commit"
    ));
    assert!(matches!(
        MergeDecision::new(
            &awaiting(),
            "merge-fixture",
            &Answer::Choice {
                choice_id: "keep".into()
            },
            MergeReadiness::Ready,
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
        assert!(
            MergeDecision::new(&awaiting(), "merge-fixture", &answer, MergeReadiness::Ready)
                .is_err()
        );
    }
    assert!(
        MergeDecision::new(&awaiting(), "stale-question", &merge, MergeReadiness::Ready).is_err()
    );
    for approval in [MergeApproval::None, approved(), resolving(TurnId::new())] {
        assert_eq!(
            MergeDecision::new(&approval, "merge-fixture", &merge, MergeReadiness::Ready)
                .err()
                .unwrap()
                .to_string(),
            "No merge is awaiting approval"
        );
    }
}

#[test]
fn active_tasks_and_failed_storage_block_both_merge_choices() {
    for readiness in [MergeReadiness::TaskActive, MergeReadiness::StorageFailed] {
        for choice in ["merge", "keep"] {
            let answer = Answer::Choice {
                choice_id: choice.into(),
            };
            assert!(MergeDecision::new(&awaiting(), "merge-fixture", &answer, readiness).is_err());
        }
    }
}

#[test]
fn recovery_preserves_live_questions_and_replaces_missing_or_mismatched_questions() {
    let approval = awaiting();
    let question = approval.question().unwrap();
    assert!(approval.recovery(&[question.clone()]).is_none());
    for pending in [
        Vec::new(),
        vec![Question {
            id: "stale-question".into(),
            ..question.clone()
        }],
        vec![Question {
            purpose: QuestionPurpose::Clarification,
            ..question.clone()
        }],
    ] {
        let event = approval.recovery(&pending).unwrap();
        let recovered = approval.transition(event).unwrap();
        let replacement = recovered.question().unwrap();
        assert_ne!(replacement.id, question.id);
        let merge = Answer::Choice {
            choice_id: "merge".into(),
        };
        assert!(
            MergeDecision::new(&recovered, &question.id, &merge, MergeReadiness::Ready).is_err()
        );
        assert!(matches!(
            MergeDecision::new(&recovered, &replacement.id, &merge, MergeReadiness::Ready).unwrap(),
            MergeDecision::Merge { commit } if commit == "approved-commit"
        ));
    }
}

#[test]
fn recovery_asks_again_after_interrupted_approval_and_preserves_resolution_state() {
    let approval = approved();
    let event = approval.recovery(&[]).unwrap();
    let recovered = approval.transition(event).unwrap();
    assert!(recovered.question().is_some());
    assert!(
        matches!(recovered, MergeApproval::Awaiting { commit, .. } if commit == "approved-commit")
    );
    let resolving = resolving(TurnId::new());
    let paused = resolving.transition(MergeEvent::Paused).unwrap();
    for approval in [MergeApproval::None, resolving, paused] {
        assert!(approval.recovery(&[]).is_none());
    }
}
