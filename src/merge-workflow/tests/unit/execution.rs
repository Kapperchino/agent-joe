use super::*;
use common_models::interaction::{Planning, WorkMode};
use common_models::tui_models::ActorToTuiPacket;
use interaction::{InteractionEvent, InteractionState, policy::InteractionPolicy};
use std::cell::Cell;

struct UnavailableStorage {
    reads: Cell<usize>,
}

impl InteractionPersistence for UnavailableStorage {
    fn ready(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn commit(&mut self, _: InteractionEvent) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Fixture storage failure"))
    }

    fn fail(&mut self, _: anyhow::Error) {}

    fn report(&self, _: ActorToTuiPacket) {}
}

impl MergePersistence for UnavailableStorage {
    fn worktree(&self) -> anyhow::Result<MergeWorktree> {
        self.reads.set(self.reads.get() + 1);
        Err(anyhow::anyhow!("Fixture snapshot failure"))
    }

    fn record_approval(&mut self, _: MergeApproval) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("Fixture storage failure"))
    }

    fn clear_worktree(&mut self) {}
}

#[test]
fn unavailable_offers_skip_snapshot_access_but_eligible_offers_propagate_errors() {
    let project = Arc::new(WorkspacePolicy::workspace(std::env::current_dir().unwrap()).unwrap());
    let workspace = workspace_access::Workspace::new(4);
    let scope = utils::execution::ExecutionScope::default();
    let policy = InteractionPolicy::default();
    let mut state = InteractionState::default();
    let mut interaction = InteractionControl {
        state: &mut state,
        persistence: UnavailableStorage {
            reads: Cell::new(0),
        },
        policy: &policy,
        role: InteractionRole::Root,
    };
    let mut environment = MergeEnvironment {
        project: Some(&project),
        workspace: &workspace,
        scope: &scope,
    };
    for readiness in [MergeReadiness::TaskActive, MergeReadiness::StorageFailed] {
        assert!(
            MergeWorkspace::for_offer(&environment, &interaction, readiness, RequestMode::Continue)
                .unwrap()
                .is_none()
        );
    }
    for mode in [RequestMode::Compact, RequestMode::SingleResponse] {
        assert!(
            MergeWorkspace::for_offer(&environment, &interaction, MergeReadiness::Ready, mode)
                .unwrap()
                .is_none()
        );
    }
    interaction.role = InteractionRole::Delegated;
    assert!(
        MergeWorkspace::for_offer(
            &environment,
            &interaction,
            MergeReadiness::Ready,
            RequestMode::Continue
        )
        .unwrap()
        .is_none()
    );
    interaction.role = InteractionRole::Root;
    environment.project = None;
    assert!(
        MergeWorkspace::for_offer(
            &environment,
            &interaction,
            MergeReadiness::Ready,
            RequestMode::Continue
        )
        .unwrap()
        .is_none()
    );
    environment.project = Some(&project);
    policy.set(
        &Planning {
            mode: WorkMode::Plan,
            ..Default::default()
        },
        &Default::default(),
    );
    assert!(
        MergeWorkspace::for_offer(
            &environment,
            &interaction,
            MergeReadiness::Ready,
            RequestMode::Continue
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(interaction.persistence.reads.get(), 0);

    policy.set(&Planning::default(), &Default::default());
    assert!(
        MergeWorkspace::for_offer(
            &environment,
            &interaction,
            MergeReadiness::Ready,
            RequestMode::Continue
        )
        .is_err()
    );
    assert_eq!(interaction.persistence.reads.get(), 1);
}
