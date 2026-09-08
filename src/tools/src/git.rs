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
pub struct GitInput {
    #[tool(description = "status, diff, show, or log", required)]
    pub operation: String,
    #[tool(description = "Optional literal file path for diff or show")]
    pub path: Option<String>,
    #[tool(description = "Revision for show or log; defaults to HEAD")]
    pub revision: Option<String>,
    #[tool(description = "For diff: staged, unstaged (default), or head")]
    pub target: Option<String>,
    #[tool(description = "For log: maximum commits, 1 through 100; default 20")]
    pub limit: Option<usize>,
}

impl TryFrom<GitInput> for GitOperation {
    type Error = anyhow::Error;
    fn try_from(input: GitInput) -> anyhow::Result<Self> {
        match input {
            GitInput {
                operation,
                path: None,
                revision: None,
                target: None,
                limit: None,
            } if operation == "status" => Ok(Self::Status),
            GitInput {
                operation,
                path,
                revision: None,
                target,
                limit: None,
            } if operation == "diff" => {
                let target = match target.as_deref().unwrap_or("unstaged") {
                    "staged" => Ok(DiffTarget::Staged),
                    "unstaged" => Ok(DiffTarget::Unstaged),
                    "head" => Ok(DiffTarget::Head),
                    _ => Err(anyhow::anyhow!(
                        "Diff target must be staged, unstaged, or head"
                    )),
                }?;
                Ok(Self::Diff {
                    target,
                    path: path.map(PathBuf::from),
                })
            }
            GitInput {
                operation,
                path,
                revision,
                target: None,
                limit: None,
            } if operation == "show" => Ok(Self::Show {
                revision: Revision::new(revision.as_deref().unwrap_or("HEAD"))?,
                path: path.map(PathBuf::from),
            }),
            GitInput {
                operation,
                path: None,
                revision,
                target: None,
                limit,
            } if operation == "log" => Ok(Self::Log {
                revision: Revision::new(revision.as_deref().unwrap_or("HEAD"))?,
                limit: LogLimit::new(limit.unwrap_or(20))?,
            }),
            _ => Err(anyhow::anyhow!(
                "Invalid Git operation or options for that operation"
            )),
        }
    }
}

impl Display for Git {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- git {}", self.input.operation)
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
