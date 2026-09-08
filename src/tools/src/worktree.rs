use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::{
    git::{
        Revision,
        worktrees::{DirtySource, ManagedWorktree, WorktreeOperation},
    },
    utils::FnvHashMap,
};

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "worktree",
    description = "Manage an isolated Git worktree inside .joe-worktrees. Create requires an explicit base revision; dirty source defaults to reject, or base_only explicitly starts from the commit without copying existing edits. Returned paths can be used for independent task work. Integrate preflights source conflicts and journals file changes, preserving the source index. Remove only cleans an unchanged base or the last integrated result; extra ignored files/private sessions prevent cleanup. Interrupted management retains its record for inspection. Creation/integration/removal require the original repository root."
)]
pub struct Worktree {
    #[tool(input)]
    pub input: WorktreeInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct WorktreeInput {
    #[tool(description = "create, list, integrate, or remove", required)]
    pub operation: String,
    #[tool(description = "Recorded managed worktree ID for integrate or remove")]
    pub id: Option<String>,
    #[tool(description = "Explicit base commit or revision for create")]
    pub base: Option<String>,
    #[tool(description = "For create: reject (default) or base_only")]
    pub dirty_source: Option<String>,
}

impl TryFrom<WorktreeInput> for WorktreeOperation {
    type Error = anyhow::Error;
    fn try_from(input: WorktreeInput) -> anyhow::Result<Self> {
        match input {
            WorktreeInput {
                operation,
                id: None,
                base: None,
                dirty_source: None,
            } if operation == "list" => Ok(Self::List),
            WorktreeInput {
                operation,
                id: None,
                base: Some(base),
                dirty_source,
            } if operation == "create" => {
                let dirty = match dirty_source.as_deref().unwrap_or("reject") {
                    "reject" => Ok(DirtySource::Reject),
                    "base_only" => Ok(DirtySource::BaseOnly),
                    _ => Err(anyhow::anyhow!(
                        "Dirty source policy must be reject or base_only"
                    )),
                }?;
                Ok(Self::Create {
                    base: Revision::new(&base)?,
                    dirty,
                })
            }
            WorktreeInput {
                operation,
                id: Some(id),
                base: None,
                dirty_source: None,
            } if operation == "integrate" => Ok(Self::Integrate { id }),
            WorktreeInput {
                operation,
                id: Some(id),
                base: None,
                dirty_source: None,
            } if operation == "remove" => Ok(Self::Remove { id }),
            _ => Err(anyhow::anyhow!("Invalid worktree operation or options")),
        }
    }
}

impl Display for Worktree {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- worktree {}", self.input.operation)
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for Worktree {
    type Input = WorktreeInput;
    type Output = Vec<ManagedWorktree>;
    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &C,
        _: &A,
    ) -> anyhow::Result<Self::Output> {
        let operation = WorktreeOperation::try_from(input)?;
        let scope = utils::execution::ExecutionScope::current();
        if let WorktreeOperation::Integrate { id } = &operation {
            let record = scope
                .changes
                .snapshot()?
                .worktrees
                .into_iter()
                .find(|record| &record.id == id)
                .ok_or_else(|| anyhow::anyhow!("Unknown managed worktree ID"))?;
            let workspace = scope.workspace()?;
            let paths = record.integration_paths(&workspace)?;
            context.prepare_edit(&paths)?;
        }
        let changes = scope.changes;
        let result = utils::files::operation(move |workspace| {
            ManagedWorktree::execute(workspace, &changes, operation)
        })
        .await?;
        context.refresh_workspace().await?;
        Ok(result)
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
    fn tool_type() -> ToolType {
        ToolType::Client
    }
    fn effect_from_input(input: &Self::Input) -> crate::tool_defs::ToolEffect {
        match input.operation.as_str() {
            "list" => crate::tool_defs::ToolEffect::Read,
            _ => crate::tool_defs::ToolEffect::Write,
        }
    }
}
