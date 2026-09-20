use crate::{MergeApproval, MergeDecision, MergeEvent, MergeProposal, MergeReadiness};
use clients::llm::LLmClient;
use clients::response::RequestMode;
use common_models::{
    interaction::{Answer, QuestionPurpose},
    runtime_ids::TurnId,
    tui_models::ActorToTuiPacket,
};
use interaction::access::InteractionRole;
use interaction::control::{InteractionControl, InteractionPersistence};
use std::sync::Arc;
use tools::tool_defs::ToolEffect;
use turn_engine::turn::FollowUp;
use utils::{
    git::worktrees::session::{CommitMessage, MergeConflict, MergeOutcome, SessionWorktree},
    workspace::WorkspacePolicy,
};

#[derive(Clone, Copy)]
pub enum MergeActivity {
    Idle,
    Active,
}

pub enum MergeWorktree {
    Inactive,
    Shared,
    Isolated(SessionWorktree),
}

pub trait MergePersistence: InteractionPersistence {
    fn worktree(&self) -> anyhow::Result<MergeWorktree>;
    fn record_approval(&mut self, approval: MergeApproval) -> anyhow::Result<()>;
    fn clear_worktree(&mut self);
}

#[derive(Clone, Copy)]
pub struct MergeEnvironment<'a> {
    pub project: Option<&'a Arc<WorkspacePolicy>>,
    pub workspace: &'a workspace_access::Workspace,
    pub scope: &'a utils::execution::ExecutionScope,
    pub request_timeout: std::time::Duration,
}

struct MergeWorkspace {
    project: Arc<WorkspacePolicy>,
    worktree: SessionWorktree,
}

struct ProposedCommit {
    workspace: MergeWorkspace,
    commit: String,
}

enum MergeResult {
    Cleaned {
        message: String,
    },
    Retained {
        message: String,
        error: anyhow::Error,
    },
    Conflicted {
        conflict: MergeConflict,
    },
}

impl MergeWorkspace {
    fn for_offer<P: MergePersistence>(
        environment: &MergeEnvironment<'_>,
        interaction: &InteractionControl<'_, P>,
        readiness: MergeReadiness,
        mode: RequestMode,
    ) -> anyhow::Result<Option<Self>> {
        match (mode, readiness, interaction.role, environment.project) {
            (
                RequestMode::Continue,
                MergeReadiness::Ready,
                InteractionRole::Root,
                Some(project),
            ) => match interaction.policy.authorize(ToolEffect::Write) {
                Ok(()) => Ok(match interaction.persistence.worktree()? {
                    MergeWorktree::Isolated(worktree) => Some(Self {
                        project: project.clone(),
                        worktree,
                    }),
                    MergeWorktree::Inactive | MergeWorktree::Shared => None,
                }),
                Err(_) => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn new<P: MergePersistence>(
        environment: &MergeEnvironment<'_>,
        interaction: &InteractionControl<'_, P>,
    ) -> anyhow::Result<Self> {
        interaction
            .persistence
            .ready()
            .map_err(|_| anyhow::anyhow!("Session storage failed; merge cannot continue"))?;
        interaction.policy.authorize(ToolEffect::Write)?;
        let project = environment
            .project
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
        match interaction.persistence.worktree()? {
            MergeWorktree::Isolated(worktree) => Ok(Self { project, worktree }),
            MergeWorktree::Inactive => Err(anyhow::anyhow!("No active session")),
            MergeWorktree::Shared => Err(anyhow::anyhow!("No session worktree")),
        }
    }

    fn proposal(&self, approval: &MergeApproval) -> anyhow::Result<Option<String>> {
        match approval {
            MergeApproval::Resolving { conflict, .. } => self
                .worktree
                .finish_resolution(&self.project, conflict)
                .map(Some),
            _ => self.worktree.proposal(&self.project),
        }
    }

    fn merge(self, commit: &str) -> anyhow::Result<MergeResult> {
        match self.worktree.merge(&self.project, commit) {
            Ok(outcome) => {
                let message = match outcome {
                    MergeOutcome::Unchanged => "Session changes are already in main.".into(),
                    MergeOutcome::Merged { target, commit } => {
                        format!("Merged session into {target}: {commit}")
                    }
                };
                Ok(match self.worktree.cleanup(&self.project, commit) {
                    Ok(()) => MergeResult::Cleaned { message },
                    Err(error) => MergeResult::Retained { message, error },
                })
            }
            Err(error) => error
                .downcast::<MergeConflict>()
                .map(|conflict| MergeResult::Conflicted { conflict }),
        }
    }
}

pub struct SessionMerge<'a, P: MergePersistence> {
    pub approval: &'a mut MergeApproval,
    pub interaction: InteractionControl<'a, P>,
    pub environment: MergeEnvironment<'a>,
    pub activity: MergeActivity,
}

pub struct MergeCompletion {
    pub message: String,
    pub action: MergeAction,
}

pub enum MergeAction {
    None,
    Relocate(WorkspacePolicy),
    Resolve { turn: TurnId },
}

impl MergeCompletion {
    fn message(message: String) -> Self {
        Self {
            message,
            action: MergeAction::None,
        }
    }
}

impl<P: MergePersistence> SessionMerge<'_, P> {
    fn readiness(&self) -> MergeReadiness {
        match (self.activity, self.interaction.persistence.ready()) {
            (_, Err(_)) => MergeReadiness::StorageFailed,
            (MergeActivity::Active, Ok(())) => MergeReadiness::TaskActive,
            (MergeActivity::Idle, Ok(())) => MergeReadiness::Ready,
        }
    }

    pub async fn offer_merge(
        &mut self,
        turn: TurnId,
        mode: RequestMode,
        client: &LLmClient,
        cache_key: &str,
    ) -> anyhow::Result<Option<MergeCompletion>> {
        match MergeWorkspace::for_offer(
            &self.environment,
            &self.interaction,
            self.readiness(),
            mode,
        )? {
            None => Ok(None),
            Some(workspace) => {
                let runtime = &self.environment;
                let lease = runtime
                    .workspace
                    .acquire(ToolEffect::Write, runtime.scope)
                    .await?;
                let approval = self.approval.clone();
                let commit = self
                    .describe_merge(workspace, approval, client, cache_key)
                    .await?;
                drop(lease);
                let proposal = MergeProposal::new(self.approval, turn, commit);
                self.record_merge(proposal.event())?;
                match proposal {
                    MergeProposal::Approved { commit } => self.merge_commit(commit).await.map(Some),
                    MergeProposal::Empty | MergeProposal::AwaitingApproval { .. } => {
                        self.interaction.refresh_interaction();
                        Ok(None)
                    }
                }
            }
        }
    }

    async fn describe_merge(
        &self,
        workspace: MergeWorkspace,
        approval: MergeApproval,
        client: &LLmClient,
        cache_key: &str,
    ) -> anyhow::Result<Option<String>> {
        let proposal = tokio::task::spawn_blocking(move || {
            let commit = workspace.proposal(&approval)?;
            Ok::<_, anyhow::Error>(commit.map(|commit| ProposedCommit { workspace, commit }))
        })
        .await??;
        match proposal {
            None => Ok(None),
            Some(ProposedCommit { workspace, commit }) => {
                let described = workspace.worktree.clone();
                let project = workspace.project.clone();
                let expected = commit.clone();
                let diff = tokio::task::spawn_blocking(move || {
                    described.proposal_diff(&project, &expected)
                })
                .await?;
                let message = match diff {
                    Ok(diff) if diff.is_empty() => {
                        CommitMessage::new("Merge session changes already present in main")
                    }
                    Ok(diff) => {
                        crate::commit_message::generate(
                            client.snapshot(),
                            diff,
                            self.environment.request_timeout,
                            Some(format!("{cache_key}:commit")),
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                match message {
                    Ok(message) => tokio::task::spawn_blocking(move || {
                        workspace
                            .worktree
                            .describe_proposal(&workspace.project, &commit, &message)
                    })
                    .await?
                    .map(Some),
                    Err(error) => {
                        self.interaction.persistence.report(ActorToTuiPacket::ContextNotice(format!(
                            "Could not generate a descriptive commit message; keeping the existing commit message: {error:#}"
                        )));
                        Ok(Some(commit))
                    }
                }
            }
        }
    }

    pub fn merge_decision(&self, id: &str, answer: &Answer) -> anyhow::Result<MergeDecision> {
        let decision = MergeDecision::new(self.approval, id, answer, self.readiness())?;
        match &decision {
            MergeDecision::Merge { .. } => {
                MergeWorkspace::new(&self.environment, &self.interaction)?;
            }
            MergeDecision::Keep => {}
        }
        Ok(decision)
    }

    pub async fn answer_merge(
        &mut self,
        decision: MergeDecision,
    ) -> anyhow::Result<MergeCompletion> {
        match decision {
            MergeDecision::Merge { commit } => {
                self.record_merge(MergeEvent::Approved {
                    commit: commit.clone(),
                })?;
                self.merge_commit(commit).await
            }
            MergeDecision::Keep => {
                self.record_merge(MergeEvent::Finished)?;
                self.interaction.refresh_interaction();
                Ok(MergeCompletion::message(
                    "Kept changes in the session worktree.".into(),
                ))
            }
        }
    }

    async fn merge_commit(&mut self, commit: String) -> anyhow::Result<MergeCompletion> {
        let approved = commit.clone();
        let outcome = async {
            let workspace = MergeWorkspace::new(&self.environment, &self.interaction)?;
            let runtime = &self.environment;
            let lease = runtime
                .workspace
                .acquire(ToolEffect::Write, runtime.scope)
                .await?;
            let outcome = tokio::task::spawn_blocking(move || workspace.merge(&approved)).await?;
            drop(lease);
            outcome
        }
        .await;
        match outcome {
            Ok(MergeResult::Cleaned { message }) => {
                self.interaction.persistence.clear_worktree();
                self.record_merge(MergeEvent::Finished)?;
                let project = self
                    .environment
                    .project
                    .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
                let workspace = WorkspacePolicy::workspace(project.root().to_path_buf())?;
                Ok(MergeCompletion {
                    message: format!("{message}\nCleaned up the session workspace and branch."),
                    action: MergeAction::Relocate(workspace),
                })
            }
            Ok(MergeResult::Retained { message, error }) => {
                self.record_merge(MergeEvent::Proposed { commit })?;
                self.interaction.refresh_interaction();
                Ok(MergeCompletion::message(format!(
                    "{message}\nCleanup could not finish; the session workspace and branch remain recorded for inspection: {error:#}. Retry the merge after resolving the cleanup issue."
                )))
            }
            Ok(MergeResult::Conflicted { conflict }) => self.resolve_merge(conflict).await,
            Err(error) => {
                self.record_merge(MergeEvent::Proposed { commit })?;
                self.interaction.refresh_interaction();
                Err(error)
            }
        }
    }

    async fn resolve_merge(&mut self, conflict: MergeConflict) -> anyhow::Result<MergeCompletion> {
        let workspace = MergeWorkspace::new(&self.environment, &self.interaction)?;
        let turn = TurnId::new();
        self.record_merge(MergeEvent::Conflicted {
            conflict: conflict.clone(),
            turn,
        })?;
        let runtime = &self.environment;
        let lease = runtime
            .workspace
            .acquire(ToolEffect::Write, runtime.scope)
            .await?;
        tokio::task::spawn_blocking(move || {
            workspace
                .worktree
                .prepare_resolution(&workspace.project, &conflict)
        })
        .await??;
        drop(lease);
        Ok(MergeCompletion {
            message: "Resolving merge conflicts in the session worktree. Joe will merge into main when resolution succeeds; your approval already covers this work.".into(),
            action: MergeAction::Resolve { turn },
        })
    }

    pub fn merge_input(&self, turn: TurnId) -> Option<FollowUp> {
        self.approval
            .resolution(turn)
            .filter(|_| matches!(self.activity, MergeActivity::Idle))
            .map(|conflict| FollowUp {
                id: turn,
                prompt: Some(format!(
                    "The user approved merging this session into main, including resolving merge conflicts. Resolve the conflicts in the current session worktree, preserving the intended changes from both branches. The worktree includes main's nonconflicting changes and conflict markers where applicable. Session commit: {}. Main commit: {}. Conflicting paths: {}. Read the current conflicting files with read_file and inspect the original versions with git show as needed, edit the affected files, remove all conflict markers, and run focused validation. Do not ask for merge approval again. When this task completes successfully, the runtime will commit the resolution and retry merging into main automatically. If resolution is blocked, explain the blocker instead of claiming success.",
                    conflict.approved,
                    conflict.target,
                    conflict
                        .paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            })
    }

    pub fn record_merge(&mut self, event: MergeEvent) -> anyhow::Result<()> {
        let question = match self.approval.transition(event) {
            Some(approval) => {
                self.interaction
                    .withdraw_questions(QuestionPurpose::Merge)?;
                self.interaction
                    .persistence
                    .record_approval(approval.clone())?;
                *self.approval = approval;
                self.approval.question()
            }
            None => None,
        };
        if let Some(question) = question {
            self.interaction.ask_question(question)?;
        }
        self.interaction.persistence.ready()
    }

    pub fn restore_merge_question(&mut self) -> anyhow::Result<()> {
        if let Some(event) = self
            .approval
            .recovery(self.interaction.state.questions().pending())
        {
            self.record_merge(event)?;
        }
        Ok(())
    }

    pub fn pause_merge(&mut self) {
        if let Err(error) = self.record_merge(MergeEvent::Paused) {
            self.interaction.persistence.fail(error);
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/execution.rs"]
mod tests;
