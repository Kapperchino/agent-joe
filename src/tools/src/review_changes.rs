use crate::tool_defs::{ToolEffect, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "review_changes",
    description = "Review the aggregate task change before claiming completion. Includes baseline and current Git status, staged and unstaged/untracked diffs, task diffs, recorded Joe edit IDs, and concurrent user changes. Read large output through its artifact reference. A conversation fork shares the filesystem and starts its own edit ownership."
)]
pub struct ReviewChanges {
    #[tool(input)]
    pub input: ReviewChangesInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ReviewChangesInput {}

impl Display for ReviewChanges {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- review task changes")
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ReviewChanges {
    type Input = ReviewChangesInput;
    type Output = utils::changes::Review;
    async fn run(_: Self::Input, _: ToolId, _: &C, _: &A) -> anyhow::Result<Self::Output> {
        let changes = utils::execution::ExecutionScope::current().changes;
        utils::files::operation(move |workspace| changes.review(workspace)).await
    }
    fn display_input(_: &Self::Input) -> String {
        "- review task changes".into()
    }
    fn req_from_input(_: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        Ok(FnvHashMap::default())
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
