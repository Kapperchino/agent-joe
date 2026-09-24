use crate::tool_defs::{ToolId, ToolOpKind, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "review_changes",
    description = "Review the aggregate task change before claiming completion. Includes baseline and current Git status, staged and unstaged/untracked diffs, task diffs, recorded Joe edit IDs, and concurrent user changes. Identical diffs use same_as JSON pointers to another field in this result. Read large output through its artifact reference. A conversation fork shares the filesystem and starts its own edit ownership."
)]
pub struct ReviewChanges {
    #[tool(input)]
    pub input: ReviewChangesInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ReviewChangesInput {
    #[tool(
        description = "For the final review, supply a concise imperative Git commit subject from your existing task context and reviewed changes. Describe behavior, not file counts; use one plain-text line of at most 72 characters. Update it after further edits. Omit or use null for an intermediate review."
    )]
    pub commit_message: Option<String>,
}

impl ReviewChangesInput {
    fn commit_message(
        &self,
    ) -> anyhow::Result<Option<utils::git::worktrees::session::CommitMessage>> {
        self.commit_message
            .as_deref()
            .map(utils::git::worktrees::session::CommitMessage::new)
            .transpose()
    }
}

impl Display for ReviewChanges {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- review task changes")
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ReviewChanges {
    type Input = ReviewChangesInput;
    type Output = utils::changes::Review;
    async fn run(input: Self::Input, _: ToolId, _: &C, _: &A) -> anyhow::Result<Self::Output> {
        let message = input.commit_message()?;
        let changes = utils::execution::ExecutionScope::current().changes;
        utils::files::operation(move |workspace| changes.record_commit_review(workspace, message))
            .await
    }
    fn display_input(_: &Self::Input) -> String {
        "- review task changes".into()
    }
    fn req_from_input(_: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(FnvHashMap::default())
    }
    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&ReviewContent::new(output))?)
    }
    fn effect() -> ToolOpKind {
        ToolOpKind::Read
    }
    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Serialize)]
struct ReviewContent<'a> {
    index_changes: &'a [utils::changes::IndexChange],
    baseline_git: &'a Option<utils::git::GitStatus>,
    current_git: &'a Option<utils::git::GitStatus>,
    baseline_staged: ReviewDiff<'a>,
    staged: ReviewDiff<'a>,
    unstaged: ReviewDiff<'a>,
    changes: Vec<ReviewFile<'a>>,
    edits: &'a [utils::changes::EditSummary],
}

#[derive(Serialize)]
struct ReviewFile<'a> {
    path: &'a std::path::Path,
    ownership: &'a utils::changes::ChangeOwnership,
    current_fingerprint: &'a str,
    task_diff: ReviewDiff<'a>,
    joe_diff: ReviewDiff<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ReviewDiff<'a> {
    Text(&'a str),
    Reference { same_as: String },
}

#[derive(Default)]
struct ReviewDiffs<'a> {
    locations: BTreeMap<&'a str, String>,
}

impl<'a> ReviewDiffs<'a> {
    fn insert(&mut self, location: String, content: &'a str) -> ReviewDiff<'a> {
        match (content, self.locations.get(content)) {
            ("", _) => ReviewDiff::Text(content),
            (_, Some(previous)) => ReviewDiff::Reference {
                same_as: previous.clone(),
            },
            (_, None) => {
                self.locations.insert(content, location);
                ReviewDiff::Text(content)
            }
        }
    }
}

impl<'a> ReviewContent<'a> {
    fn new(review: &'a utils::changes::Review) -> Self {
        let mut diffs = ReviewDiffs::default();
        let changes = review
            .changes
            .iter()
            .enumerate()
            .map(|(index, change)| ReviewFile {
                path: &change.path,
                ownership: &change.ownership,
                current_fingerprint: &change.current_fingerprint,
                task_diff: diffs.insert(format!("/changes/{index}/task_diff"), &change.task_diff),
                joe_diff: diffs.insert(format!("/changes/{index}/joe_diff"), &change.joe_diff),
            })
            .collect();
        Self {
            index_changes: &review.index_changes,
            baseline_git: &review.baseline_git,
            current_git: &review.current_git,
            baseline_staged: diffs.insert("/baseline_staged".into(), &review.baseline_staged),
            staged: diffs.insert("/staged".into(), &review.staged),
            unstaged: diffs.insert("/unstaged".into(), &review.unstaged),
            changes,
            edits: &review.edits,
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/review_changes/tests.rs"]
mod tests;
