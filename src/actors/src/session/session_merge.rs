use crate::{
    context::RequestMode,
    session::{Event, persistence::Persistence},
    states::{
        actor_state::ActorState,
        runtime::{ExecutionRole, Runtime},
        turn::FollowUp,
        turn_machine::TurnMachine,
    },
};
use analysis::contexts::context::Context;
use common_models::{
    interaction::{Answer, Choice, Question, QuestionPurpose},
    runtime_ids::{OperationId, TurnId},
    tui_models::ActorToTuiPacket,
};
use std::sync::Arc;
use tools::tool_defs::ToolEffect;
use utils::{
    git::worktrees::session::{CommitMessage, MergeConflict, MergeOutcome, SessionWorktree},
    workspace::WorkspacePolicy,
};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum MergeApproval {
    #[default]
    None,
    Awaiting {
        question: String,
        commit: String,
    },
    Approved {
        commit: String,
    },
    Resolving {
        conflict: MergeConflict,
        #[serde(skip)]
        activity: ResolutionActivity,
    },
}

#[derive(Clone, Default)]
pub enum ResolutionActivity {
    #[default]
    Paused,
    Running {
        turn: TurnId,
    },
}

pub enum MergeEvent {
    TaskStarted {
        turn: TurnId,
    },
    Proposed {
        commit: String,
    },
    Approved {
        commit: String,
    },
    Conflicted {
        conflict: MergeConflict,
        turn: TurnId,
    },
    Paused,
    Finished,
}

enum MergeProposal {
    Empty,
    AwaitingApproval { commit: String },
    Approved { commit: String },
}

impl MergeProposal {
    fn new(approval: &MergeApproval, turn: TurnId, commit: Option<String>) -> Self {
        match commit {
            Some(commit) if approval.resolution(turn).is_some() => Self::Approved { commit },
            Some(commit) => Self::AwaitingApproval { commit },
            None => Self::Empty,
        }
    }

    fn event(&self) -> MergeEvent {
        match self {
            Self::Empty => MergeEvent::Finished,
            Self::AwaitingApproval { commit } => MergeEvent::Proposed {
                commit: commit.clone(),
            },
            Self::Approved { commit } => MergeEvent::Approved {
                commit: commit.clone(),
            },
        }
    }
}

pub(super) enum MergeDecision {
    Merge { commit: String },
    Keep,
}

impl MergeDecision {
    fn new(
        approval: &MergeApproval,
        id: &str,
        answer: &Answer,
        turn: &TurnMachine,
        persistence: &Persistence,
    ) -> anyhow::Result<Self> {
        approval
            .question()
            .filter(|question| question.id == id)
            .ok_or_else(|| anyhow::anyhow!("No merge is awaiting approval"))?
            .answer(answer)?;
        match (turn.is_idle(), persistence, approval, answer) {
            (
                true,
                Persistence::Ready,
                MergeApproval::Awaiting { commit, .. },
                Answer::Choice { choice_id },
            ) if choice_id == "merge" => Ok(Self::Merge {
                commit: commit.clone(),
            }),
            (true, Persistence::Ready, _, _) => Ok(Self::Keep),
            _ => Err(anyhow::anyhow!(
                "Finish the active task before answering the merge question"
            )),
        }
    }
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
    fn for_offer(
        runtime: &Runtime,
        turn: &TurnMachine,
        persistence: &Persistence,
        mode: RequestMode,
    ) -> anyhow::Result<Option<Self>> {
        match (
            mode,
            persistence,
            &runtime.role,
            &runtime.project,
            &runtime.session,
        ) {
            (
                RequestMode::Continue,
                Persistence::Ready,
                ExecutionRole::Root,
                Some(project),
                Some(session),
            ) if turn.is_idle() => match runtime.interaction.authorize(ToolEffect::Write) {
                Ok(()) => Ok(session.snapshot()?.worktree.map(|worktree| Self {
                    project: project.clone(),
                    worktree,
                })),
                Err(_) => Ok(None),
            },
            _ => Ok(None),
        }
    }

    fn new(runtime: &Runtime, persistence: &Persistence) -> anyhow::Result<Self> {
        match persistence {
            Persistence::Ready => {
                runtime.interaction.authorize(ToolEffect::Write)?;
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

impl MergeApproval {
    pub fn resolution(&self, turn: TurnId) -> Option<&MergeConflict> {
        match self {
            Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn: approved },
            } if *approved == turn => Some(conflict),
            _ => None,
        }
    }

    fn transition(&self, event: MergeEvent) -> Option<Self> {
        match (self, event) {
            (Self::Awaiting { .. } | Self::Approved { .. }, MergeEvent::TaskStarted { .. }) => {
                Some(Self::None)
            }
            (
                Self::Resolving {
                    activity: ResolutionActivity::Running { turn: approved },
                    ..
                },
                MergeEvent::TaskStarted { turn },
            ) if *approved == turn => None,
            (
                Self::Resolving {
                    conflict,
                    activity: ResolutionActivity::Running { .. },
                },
                MergeEvent::TaskStarted { .. } | MergeEvent::Paused,
            ) => Some(Self::Resolving {
                conflict: conflict.clone(),
                activity: ResolutionActivity::Paused,
            }),
            (_, MergeEvent::TaskStarted { .. } | MergeEvent::Paused) => None,
            (_, MergeEvent::Proposed { commit }) => Some(Self::Awaiting {
                question: format!("merge-{}", OperationId::new()),
                commit,
            }),
            (_, MergeEvent::Approved { commit }) => Some(Self::Approved { commit }),
            (_, MergeEvent::Conflicted { conflict, turn }) => Some(Self::Resolving {
                conflict,
                activity: ResolutionActivity::Running { turn },
            }),
            (_, MergeEvent::Finished) => Some(Self::None),
        }
    }

    pub fn question(&self) -> Option<Question> {
        match self {
            Self::None | Self::Approved { .. } | Self::Resolving { .. } => None,
            Self::Awaiting { question, commit } => Some(Question {
                purpose: QuestionPurpose::Merge,
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
    pub async fn offer_merge(&mut self, turn: TurnId) -> anyhow::Result<()> {
        if let Some(workspace) = MergeWorkspace::for_offer(
            &self.runtime,
            &self.turn,
            &self.persistence,
            self.conversation.request_mode(turn, self.request_mode),
        )? {
            let runtime = &self.runtime;
            let lease = runtime
                .workspace
                .acquire(ToolEffect::Write, &runtime.scope)
                .await?;
            let approval = self.merge_approval.clone();
            let commit = self.describe_merge(workspace, approval).await?;
            drop(lease);
            let proposal = MergeProposal::new(&self.merge_approval, turn, commit);
            self.record_merge(proposal.event())?;
            match proposal {
                MergeProposal::Approved { commit } => {
                    let message = self.merge_commit(commit).await?;
                    self.reporter.send(ActorToTuiPacket::ContextNotice(message));
                }
                MergeProposal::Empty | MergeProposal::AwaitingApproval { .. } => {
                    self.refresh_interaction()
                }
            }
        }
        Ok(())
    }

    async fn describe_merge(
        &self,
        workspace: MergeWorkspace,
        approval: MergeApproval,
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
                            self.llm.snapshot(),
                            diff,
                            self.runtime.request_timeout,
                            Some(format!("{}:commit", self.conversation.cache_key())),
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
                        self.reporter.send(ActorToTuiPacket::ContextNotice(format!(
                            "Could not generate a descriptive commit message; keeping the existing commit message: {error:#}"
                        )));
                        Ok(Some(commit))
                    }
                }
            }
        }
    }

    pub(super) fn merge_decision(
        &self,
        id: &str,
        answer: &Answer,
    ) -> anyhow::Result<MergeDecision> {
        let decision = MergeDecision::new(
            &self.merge_approval,
            id,
            answer,
            &self.turn,
            &self.persistence,
        )?;
        match &decision {
            MergeDecision::Merge { .. } => {
                MergeWorkspace::new(&self.runtime, &self.persistence)?;
            }
            MergeDecision::Keep => {}
        }
        Ok(decision)
    }

    pub(super) async fn answer_merge(&mut self, decision: MergeDecision) -> anyhow::Result<String> {
        match decision {
            MergeDecision::Merge { commit } => {
                self.record_merge(MergeEvent::Approved {
                    commit: commit.clone(),
                })?;
                self.merge_commit(commit).await
            }
            MergeDecision::Keep => {
                self.record_merge(MergeEvent::Finished)?;
                self.refresh_interaction();
                Ok("Kept changes in the session worktree.".into())
            }
        }
    }

    async fn merge_commit(&mut self, commit: String) -> anyhow::Result<String> {
        let approved = commit.clone();
        let outcome = async {
            let workspace = MergeWorkspace::new(&self.runtime, &self.persistence)?;
            let runtime = &self.runtime;
            let lease = runtime
                .workspace
                .acquire(ToolEffect::Write, &runtime.scope)
                .await?;
            let outcome = tokio::task::spawn_blocking(move || workspace.merge(&approved)).await?;
            drop(lease);
            outcome
        }
        .await;
        match outcome {
            Ok(MergeResult::Cleaned { message }) => {
                self.persist(Event::Worktree(None));
                self.record_merge(MergeEvent::Finished)?;
                let mut runtime = self.runtime.clone();
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
            Ok(MergeResult::Retained { message, error }) => {
                self.record_merge(MergeEvent::Proposed { commit })?;
                self.refresh_interaction();
                Ok(format!(
                    "{message}\nCleanup could not finish; the session workspace and branch remain recorded for inspection: {error:#}. Retry the merge after resolving the cleanup issue."
                ))
            }
            Ok(MergeResult::Conflicted { conflict }) => self.resolve_merge(conflict).await,
            Err(error) => {
                self.record_merge(MergeEvent::Proposed { commit })?;
                self.refresh_interaction();
                Err(error)
            }
        }
    }

    async fn resolve_merge(&mut self, conflict: MergeConflict) -> anyhow::Result<String> {
        let workspace = MergeWorkspace::new(&self.runtime, &self.persistence)?;
        let turn = TurnId::new();
        self.record_merge(MergeEvent::Conflicted {
            conflict: conflict.clone(),
            turn,
        })?;
        let runtime = &self.runtime;
        let lease = runtime
            .workspace
            .acquire(ToolEffect::Write, &runtime.scope)
            .await?;
        tokio::task::spawn_blocking(move || {
            workspace
                .worktree
                .prepare_resolution(&workspace.project, &conflict)
        })
        .await??;
        drop(lease);
        self.context.refresh_workspace().await?;
        self.refresh_interaction();
        self.actor_ref
            .send_message(crate::actor::Message::ResolveMerge { turn })?;
        Ok("Resolving merge conflicts in the session worktree. Joe will merge into main when resolution succeeds; your approval already covers this work.".into())
    }

    pub fn merge_input(&self, turn: TurnId) -> Option<FollowUp> {
        self.merge_approval
            .resolution(turn)
            .filter(|_| self.turn.is_idle())
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
        let question = match self.merge_approval.transition(event) {
            Some(approval) => {
                self.withdraw_questions(QuestionPurpose::Merge)?;
                self.commit_interaction(Event::MergeApproval(approval.clone()))?;
                self.merge_approval = approval;
                self.merge_approval.question()
            }
            None => None,
        };
        if let Some(question) = question {
            self.ask_question(question)?;
        }
        match &self.persistence {
            Persistence::Ready => Ok(()),
            Persistence::Failed(failure) => Err(anyhow::anyhow!(failure.to_string())),
        }
    }

    pub fn restore_merge_question(&mut self) -> anyhow::Result<()> {
        let commit = match &self.merge_approval {
            MergeApproval::Awaiting { question, commit }
                if !self
                    .interaction
                    .questions()
                    .pending()
                    .iter()
                    .any(|pending| {
                        pending.id == *question && pending.purpose == QuestionPurpose::Merge
                    }) =>
            {
                Some(commit.clone())
            }
            MergeApproval::Approved { commit } => Some(commit.clone()),
            _ => None,
        };
        if let Some(commit) = commit {
            self.record_merge(MergeEvent::Proposed { commit })?;
        }
        Ok(())
    }

    pub fn pause_merge(&mut self) {
        if let Err(error) = self.record_merge(MergeEvent::Paused) {
            self.persistence_failed(error);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/session_merge/tests.rs"]
mod tests;
