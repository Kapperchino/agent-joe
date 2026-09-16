use crate::{
    actor_state::ActorState, runtime::Runtime, session::Event, session_control::Persistence,
    turn::FollowUp, turn_machine::TurnMachine,
};
use analysis::contexts::context::Context;
use common_models::{
    interaction::{Answer, Choice, Question},
    runtime_ids::{OperationId, TurnId},
    tui_models::ActorToTuiPacket,
};
use std::sync::Arc;
use utils::{
    git::worktrees::session::{MergeConflict, MergeOutcome, SessionWorktree},
    workspace::WorkspacePolicy,
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) enum MergeApproval {
    #[default]
    None,
    Awaiting {
        question: String,
        commit: String,
    },
    Resolving {
        conflict: MergeConflict,
        #[serde(skip)]
        activity: ResolutionActivity,
    },
}

#[derive(Clone, Default)]
pub(crate) enum ResolutionActivity {
    #[default]
    Paused,
    Running {
        turn: TurnId,
    },
}

pub(crate) enum MergeEvent {
    TaskStarted {
        turn: TurnId,
    },
    Proposed {
        commit: String,
    },
    Conflicted {
        conflict: MergeConflict,
        turn: TurnId,
    },
    Paused,
    Finished,
}

enum MergeContinuation {
    Ask,
    Merge { commit: String },
}

enum MergeDecision {
    Merge { commit: String },
    Keep,
}

impl MergeDecision {
    fn new(
        approval: &MergeApproval,
        answer: &Answer,
        turn: &TurnMachine,
        persistence: &Persistence,
    ) -> anyhow::Result<Self> {
        approval
            .question()
            .ok_or_else(|| anyhow::anyhow!("No merge is awaiting approval"))?
            .answer(answer)?;
        match approval {
            _ if !turn.is_idle() || !matches!(persistence, Persistence::Ready) => Err(
                anyhow::anyhow!("Finish the active task before answering the merge question"),
            ),
            MergeApproval::Awaiting { commit, .. } if matches!(answer, Answer::Choice { choice_id } if choice_id == "merge") => {
                Ok(Self::Merge {
                    commit: commit.clone(),
                })
            }
            _ => Ok(Self::Keep),
        }
    }
}

struct MergeWorkspace {
    project: Arc<WorkspacePolicy>,
    worktree: SessionWorktree,
}

enum CompletedMerge {
    Cleaned {
        message: String,
    },
    Retained {
        message: String,
        error: anyhow::Error,
    },
}

impl MergeWorkspace {
    fn new(runtime: &Runtime, persistence: &Persistence) -> anyhow::Result<Self> {
        match persistence {
            Persistence::Ready => {
                runtime
                    .interaction
                    .authorize(tools::tool_defs::ToolEffect::Write)?;
                let project = runtime
                    .project
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
                let session = runtime
                    .session
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No active session"))?;
                let worktree = session
                    .snapshot()?
                    .worktree
                    .ok_or_else(|| anyhow::anyhow!("No session worktree"))?;
                Ok(Self { project, worktree })
            }
            Persistence::Failed(_) => Err(anyhow::anyhow!(
                "Session storage failed; merge cannot continue"
            )),
        }
    }

    fn merge(self, commit: &str) -> anyhow::Result<CompletedMerge> {
        let message = match self.worktree.merge(&self.project, commit)? {
            MergeOutcome::Unchanged => "Session changes are already in main.".into(),
            MergeOutcome::Merged { target, commit } => {
                format!("Merged session into {target}: {commit}")
            }
        };
        Ok(match self.worktree.cleanup(&self.project, commit) {
            Ok(()) => CompletedMerge::Cleaned { message },
            Err(error) => CompletedMerge::Retained { message, error },
        })
    }
}

impl MergeApproval {
    pub(crate) fn resolution(&self, turn: TurnId) -> Option<&MergeConflict> {
        match self {
            Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn: approved },
            } if *approved == turn => Some(conflict),
            _ => None,
        }
    }

    fn transition(&self, event: MergeEvent) -> Option<Self> {
        match event {
            MergeEvent::TaskStarted { .. } if matches!(self, Self::Awaiting { .. }) => {
                Some(Self::None)
            }
            MergeEvent::TaskStarted { turn } if self.resolution(turn).is_none() => {
                self.transition(MergeEvent::Paused)
            }
            MergeEvent::TaskStarted { .. } => None,
            MergeEvent::Proposed { commit } => Some(Self::Awaiting {
                question: format!("merge-{}", OperationId::new()),
                commit,
            }),
            MergeEvent::Conflicted { conflict, turn } => Some(Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn },
            }),
            MergeEvent::Paused => match self {
                Self::Resolving {
                    conflict,
                    activity: ResolutionActivity::Running { .. },
                } => Some(Self::Resolving {
                    conflict: conflict.clone(),
                    activity: ResolutionActivity::Paused,
                }),
                _ => None,
            },
            MergeEvent::Finished => Some(Self::None),
        }
    }

    fn continuation(&self, turn: TurnId, commit: &Option<String>) -> MergeContinuation {
        match commit {
            Some(commit) if self.resolution(turn).is_some() => MergeContinuation::Merge {
                commit: commit.clone(),
            },
            _ => MergeContinuation::Ask,
        }
    }

    pub(crate) fn question(&self) -> Option<Question> {
        match self {
            Self::None | Self::Resolving { .. } => None,
            Self::Awaiting { question, commit } => Some(Question {
                id: question.clone(),
                prompt: format!(
                    "Task completed successfully. Merge session commit {commit} into main, resolving any conflicts, then delete the entire session workspace, including ignored files and build caches?"
                ),
                required: false,
                choices: vec![
                    Choice {
                        id: "merge".into(),
                        label: "Merge into main".into(),
                    },
                    Choice {
                        id: "keep".into(),
                        label: "Keep changes in this session".into(),
                    },
                ],
                allow_free_text: false,
            }),
        }
    }
}

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) async fn offer_merge(&mut self, turn: TurnId) -> anyhow::Result<()> {
        if matches!(
            self.dependency.runtime.role,
            crate::runtime::ExecutionRole::Root
        ) && self.turn.is_idle()
            && self
                .dependency
                .runtime
                .interaction
                .authorize(tools::tool_defs::ToolEffect::Write)
                .is_ok()
            && self.compact_turn != Some(turn)
            && matches!(self.persistence, Persistence::Ready)
            && let (Some(project), Some(session)) = (
                &self.dependency.runtime.project,
                &self.dependency.runtime.session,
            )
            && let Some(worktree) = session.snapshot()?.worktree
        {
            let runtime = &self.dependency.runtime;
            let lease = runtime
                .workspace
                .acquire(tools::tool_defs::ToolEffect::Write, &runtime.scope)
                .await?;
            let project = project.clone();
            let approval = self.merge_approval.clone();
            let commit = tokio::task::spawn_blocking(move || match approval {
                MergeApproval::Resolving { conflict, .. } => {
                    worktree.finish_resolution(&project, &conflict).map(Some)
                }
                _ => worktree.proposal(&project),
            })
            .await??;
            drop(lease);
            let continuation = self.merge_approval.continuation(turn, &commit);
            self.record_merge(match commit {
                Some(commit) => MergeEvent::Proposed { commit },
                None => MergeEvent::Finished,
            })?;
            match continuation {
                MergeContinuation::Merge { commit } => {
                    let message = self.merge_commit(commit).await?;
                    self.reporter.send(ActorToTuiPacket::ContextNotice(message));
                }
                MergeContinuation::Ask => self.refresh_interaction(),
            }
        }
        Ok(())
    }

    pub(crate) async fn answer_merge(&mut self, answer: &Answer) -> anyhow::Result<String> {
        match MergeDecision::new(&self.merge_approval, answer, &self.turn, &self.persistence)? {
            MergeDecision::Merge { commit } => self.merge_commit(commit).await,
            MergeDecision::Keep => {
                self.record_merge(MergeEvent::Finished)?;
                self.refresh_interaction();
                Ok("Kept changes in the session worktree.".into())
            }
        }
    }

    async fn merge_commit(&mut self, commit: String) -> anyhow::Result<String> {
        let workspace = MergeWorkspace::new(&self.dependency.runtime, &self.persistence)?;
        let runtime = &self.dependency.runtime;
        let lease = runtime
            .workspace
            .acquire(tools::tool_defs::ToolEffect::Write, &runtime.scope)
            .await?;
        runtime.scope.shutdown_sandbox().await?;
        let outcome = tokio::task::spawn_blocking(move || workspace.merge(&commit)).await?;
        drop(lease);
        match outcome {
            Ok(CompletedMerge::Cleaned { message }) => {
                self.persist(Event::Worktree(None));
                self.record_merge(MergeEvent::Finished)?;
                let mut runtime = self.dependency.runtime.clone();
                let project = runtime
                    .project
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
                runtime.scope = runtime
                    .scope
                    .relocated(WorkspacePolicy::workspace(project.root().to_path_buf())?);
                self.relocate_session_workspace(runtime).await?;
                self.refresh_interaction();
                Ok(format!(
                    "{message}\nCleaned up the session workspace and branch."
                ))
            }
            Ok(CompletedMerge::Retained { message, error }) => {
                self.refresh_interaction();
                Ok(format!(
                    "{message}\nCleanup could not finish; the session workspace and branch remain recorded for inspection: {error:#}. Retry the merge after resolving the cleanup issue."
                ))
            }
            Err(error) => match error.downcast_ref::<MergeConflict>().cloned() {
                Some(conflict) => self.resolve_merge(conflict).await,
                None => {
                    self.refresh_interaction();
                    Err(error)
                }
            },
        }
    }

    async fn resolve_merge(&mut self, conflict: MergeConflict) -> anyhow::Result<String> {
        let workspace = MergeWorkspace::new(&self.dependency.runtime, &self.persistence)?;
        let turn = TurnId::new();
        self.record_merge(MergeEvent::Conflicted {
            conflict: conflict.clone(),
            turn,
        })?;
        let runtime = &self.dependency.runtime;
        let lease = runtime
            .workspace
            .acquire(tools::tool_defs::ToolEffect::Write, &runtime.scope)
            .await?;
        tokio::task::spawn_blocking(move || {
            workspace
                .worktree
                .prepare_resolution(&workspace.project, &conflict)
        })
        .await??;
        drop(lease);
        self.cur_context.refresh_workspace().await?;
        self.refresh_interaction();
        self.actor_ref
            .send_message(crate::actor::Message::ResolveMerge { turn })?;
        Ok("Resolving merge conflicts in the session worktree. Joe will merge into main when resolution succeeds; your approval already covers this work.".into())
    }

    pub(crate) fn merge_input(&self, turn: TurnId) -> Option<FollowUp> {
        match self.merge_approval.resolution(turn) {
            Some(conflict) if self.turn.is_idle() => Some(FollowUp {
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
            }),
            _ => None,
        }
    }

    pub(crate) fn record_merge(&mut self, event: MergeEvent) -> anyhow::Result<()> {
        if let Some(approval) = self.merge_approval.transition(event) {
            self.merge_approval = approval;
            self.persist(Event::MergeApproval(self.merge_approval.clone()));
        }
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }
    }

    pub(crate) fn pause_merge(&mut self) {
        if let Err(error) = self.record_merge(MergeEvent::Paused) {
            self.persistence_failed(error);
        }
    }
}
