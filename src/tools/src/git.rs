use crate::tool_defs::{ToolDefTrait, ToolEffect, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::{
    git::{DiffTarget, GitOperation, GitRepository, GitResult, LogLimit, Revision},
    utils::FnvHashMap,
};

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "git",
    description = "Inspect Git using status, diff, show, or log. Returns structured results. Paths are literal project paths; revisions are commit IDs or simple refs with optional ancestry. No shell, pager, external diff, textconv, hook, network, staging, or commit execution. Diff includes untracked files. Use review_changes to distinguish task edits from existing changes before finishing."
)]
pub struct Git {
    #[tool(input)]
    pub input: GitInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
#[serde(tag = "operation", rename_all = "snake_case")]
#[tool(
    description = "Git operation. Fields belonging to other operations are ignored. Omit or use null for unused options; empty strings use defaults."
)]
pub enum GitInput {
    #[default]
    Status,
    Diff {
        #[tool(description = "Literal file path for diff or show; omit for all paths.")]
        path: Option<String>,
        #[tool(
            description = "Diff target; defaults to unstaged.",
            values("staged", "unstaged", "head")
        )]
        target: Option<String>,
    },
    Show {
        #[tool(description = "Literal file path for diff or show; omit for all paths.")]
        path: Option<String>,
        #[tool(description = "Revision for show or log; defaults to HEAD.")]
        revision: Option<String>,
    },
    Log {
        #[tool(description = "Revision for show or log; defaults to HEAD.")]
        revision: Option<String>,
        #[tool(
            description = "Maximum commits for log; defaults to 20.",
            minimum = 1,
            maximum = 100
        )]
        limit: Option<usize>,
    },
}

impl GitInput {
    fn operation(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Diff { .. } => "diff",
            Self::Show { .. } => "show",
            Self::Log { .. } => "log",
        }
    }
}

impl TryFrom<GitInput> for GitOperation {
    type Error = anyhow::Error;
    fn try_from(input: GitInput) -> anyhow::Result<Self> {
        match input {
            GitInput::Status => Ok(Self::Status),
            GitInput::Diff { path, target } => Ok(Self::Diff {
                target: DiffTarget::new(optional_text(target).as_deref().unwrap_or("unstaged"))?,
                path: optional_text(path).map(PathBuf::from),
            }),
            GitInput::Show { path, revision } => Ok(Self::Show {
                revision: Revision::new(optional_text(revision).as_deref().unwrap_or("HEAD"))?,
                path: optional_text(path).map(PathBuf::from),
            }),
            GitInput::Log { revision, limit } => Ok(Self::Log {
                revision: Revision::new(optional_text(revision).as_deref().unwrap_or("HEAD"))?,
                limit: LogLimit::new(limit.unwrap_or(20))?,
            }),
        }
    }
}

fn optional_text(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

impl Display for Git {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- git {}", self.input.operation())
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for Git {
    type Input = GitInput;
    type Output = GitResult;
    async fn run(input: Self::Input, _: ToolId, _: &C, _: &A) -> anyhow::Result<Self::Output> {
        let operation = GitOperation::try_from(input)?;
        utils::files::operation(move |workspace| GitRepository::execute(workspace, operation)).await
    }
    fn display_input(input: &Self::Input) -> String {
        Self {
            input: input.clone(),
        }
        .to_string()
    }
    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Self {
            input: input.clone(),
        }
        .req()
    }
    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }
    fn effect() -> ToolEffect {
        ToolEffect::Read
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[cfg(test)]
#[path = "../../../tests/tools/git/tests.rs"]
mod tests;
