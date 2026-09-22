use super::*;

fn step(id: &str, kind: StepKind) -> PlanStep {
    PlanStep {
        id: id.into(),
        kind,
        description: format!("Resolve {id}"),
        dependencies: Vec::new(),
        acceptance: "Behavior and affected source are understood".into(),
        state: StepState::Pending,
        evidence: Vec::new(),
        blocked_reason: None,
        validation: None,
    }
}

fn update(plan: &Plan, steps: Vec<PlanStep>) -> PlanUpdate {
    PlanUpdate {
        revision: plan.revision,
        requirements_revision: plan.requirements_revision,
        steps,
    }
}

#[test]
fn investigation_requires_all_research_but_leaves_implementation_pending() {
    let mut plan = Plan::default();
    assert!(matches!(plan.investigation(), Investigation::Missing));
    plan.steps.push(step("edit", StepKind::Implementation));
    assert!(matches!(plan.investigation(), Investigation::Missing));
    plan.steps.push(step("source", StepKind::Investigation));
    plan.steps.push(step("design", StepKind::Investigation));
    let evidence = BTreeMap::from([("tool:read".into(), "Inspected source and tests".into())]);
    for state in [
        StepState::Pending,
        StepState::InProgress,
        StepState::Blocked,
    ] {
        plan.steps[1].state = state;
        assert!(
            matches!(plan.investigation(), Investigation::Unfinished(steps) if steps.len() == 2)
        );
    }
    plan.steps[1].state = StepState::Pending;
    let steps = plan
        .steps
        .iter()
        .map(|step| PlanStep {
            state: match step.kind {
                StepKind::Investigation => StepState::Completed,
                StepKind::Implementation => StepState::Pending,
            },
            evidence: vec![PlanEvidence {
                source: "tool:read".into(),
                explanation: "Source and tests establish the affected behavior".into(),
            }],
            ..step.clone()
        })
        .collect();
    let plan = plan.update(update(&plan, steps), 0, &evidence).unwrap();
    assert!(matches!(plan.investigation(), Investigation::Complete));
    assert_eq!(plan.steps[0].state, StepState::Pending);
    let mut reopened = plan.steps.clone();
    reopened[2].state = StepState::Pending;
    let plan = plan.update(update(&plan, reopened), 0, &evidence).unwrap();
    assert!(
        matches!(plan.investigation(), Investigation::Unfinished(steps) if steps.len() == 1 && steps[0].id == "design")
    );
}

#[test]
fn investigation_completion_requires_recorded_evidence_and_reopening_after_changes() {
    let plan = Plan::default();
    let plan = plan
        .update(
            update(&plan, vec![step("source", StepKind::Investigation)]),
            0,
            &BTreeMap::new(),
        )
        .unwrap();
    let mut steps = plan.steps.clone();
    steps[0].state = StepState::Completed;
    assert!(
        plan.update(update(&plan, steps.clone()), 0, &BTreeMap::new())
            .is_err()
    );
    steps[0].evidence.push(PlanEvidence {
        source: "tool:read".into(),
        explanation: "Read source and checked assumptions".into(),
    });
    assert!(
        plan.update(update(&plan, steps.clone()), 0, &BTreeMap::new())
            .is_err()
    );
    let evidence = BTreeMap::from([("tool:read".into(), "Read source".into())]);
    let plan = plan.update(update(&plan, steps), 0, &evidence).unwrap();
    let changed = PlanUpdate {
        requirements_revision: 1,
        ..update(&plan, plan.steps.clone())
    };
    assert!(plan.update(changed.clone(), 1, &evidence).is_err());
    let mut changed = changed;
    changed.steps[0].state = StepState::Pending;
    let plan = plan.update(changed, 1, &evidence).unwrap();
    assert!(matches!(plan.investigation(), Investigation::Unfinished(_)));
}

#[test]
fn changing_step_kind_requires_reopening_and_investigation_cannot_run_cargo() {
    let evidence = BTreeMap::from([("tool:read".into(), "Read source".into())]);
    let mut initial = step("source", StepKind::Implementation);
    initial.state = StepState::Completed;
    initial.evidence.push(PlanEvidence {
        source: "tool:read".into(),
        explanation: "Observed the source".into(),
    });
    let plan = Plan {
        steps: vec![initial],
        ..Default::default()
    };
    let mut steps = plan.steps.clone();
    steps[0].kind = StepKind::Investigation;
    assert!(
        plan.update(update(&plan, steps.clone()), 0, &evidence)
            .is_err()
    );
    steps[0].state = StepState::Pending;
    let plan = plan.update(update(&plan, steps), 0, &evidence).unwrap();
    let mut steps = plan.steps.clone();
    steps[0].validation = Some(ValidationRequirement {
        cargo: serde_json::json!({"operation": "test"})
            .as_object()
            .unwrap()
            .clone(),
    });
    assert!(plan.update(update(&plan, steps), 0, &evidence).is_err());
}

#[test]
fn legacy_plans_remain_implementation_and_step_kinds_survive_storage() {
    let original = step("edit", StepKind::Implementation);
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("kind");
    assert_eq!(
        serde_json::from_value::<PlanStep>(legacy).unwrap(),
        original
    );
    let plan = Plan {
        steps: vec![step("source", StepKind::Investigation), original],
        ..Default::default()
    };
    let restored: Plan = serde_json::from_str(&serde_json::to_string(&plan).unwrap()).unwrap();
    assert_eq!(restored, plan);
    assert!(matches!(
        restored.investigation(),
        Investigation::Unfinished(_)
    ));
}
