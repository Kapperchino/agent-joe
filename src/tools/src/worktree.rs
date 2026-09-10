use crate::tool_defs::{NonEmptyString, ToolDefTrait, ToolEffect, ToolId, ToolTrait, ToolType};
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
#[serde(tag = "operation", rename_all = "snake_case")]
#[tool(
    description = "Worktree operation. Fields belonging to other operations are ignored. Omit or use null for unused options."
)]
pub enum WorktreeInput {
    #[default]
    List,
    Create {
        #[tool(
            description = "Required explicit base commit or revision for create.",
            kind = "string"
        )]
        base: Revision,
        #[tool(
            description = "For create: reject dirty sources by default; base_only explicitly isolates the base commit. Empty strings use the default.",
            values("reject", "base_only")
        )]
        dirty_source: Option<String>,
    },
    Integrate {
        #[tool(
            description = "Required recorded managed worktree ID for integrate or remove.",
            kind = "string"
        )]
        id: NonEmptyString,
    },
    Remove {
        #[tool(
            description = "Required recorded managed worktree ID for integrate or remove.",
            kind = "string"
        )]
        id: NonEmptyString,
    },
}

impl WorktreeInput {
    fn operation(&self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Create { .. } => "create",
            Self::Integrate { .. } => "integrate",
            Self::Remove { .. } => "remove",
        }
    }
}

impl TryFrom<WorktreeInput> for WorktreeOperation {
    type Error = anyhow::Error;

    fn try_from(input: WorktreeInput) -> anyhow::Result<Self> {
        match input {
            WorktreeInput::List => Ok(Self::List),
            WorktreeInput::Create { base, dirty_source } => Ok(Self::Create {
                base,
                dirty: DirtySource::new(
                    dirty_source
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .unwrap_or("reject"),
                )?,
            }),
            WorktreeInput::Integrate { id } => Ok(Self::Integrate { id: id.into() }),
            WorktreeInput::Remove { id } => Ok(Self::Remove { id: id.into() }),
        }
    }
}

impl Display for Worktree {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- worktree {}", self.input.operation())
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
        match &operation {
            WorktreeOperation::Integrate { id } => {
                let workspace = scope.workspace()?;
                let record = ManagedWorktree::selected(&workspace, &scope.changes, id)?;
                context.prepare_edit(&record.integration_paths(&workspace)?)?;
            }
            WorktreeOperation::Create { .. }
            | WorktreeOperation::List
            | WorktreeOperation::Remove { .. } => {}
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

    fn effect_from_input(input: &Self::Input) -> ToolEffect {
        match input {
            WorktreeInput::List => ToolEffect::Read,
            WorktreeInput::Create { .. }
            | WorktreeInput::Integrate { .. }
            | WorktreeInput::Remove { .. } => ToolEffect::Write,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_defs::LenientDeserialize;
    use serde_json::{Value, json};

    #[test]
    fn unused_fields_do_not_prevent_listing_worktrees() {
        let input = WorktreeInput::deserialize_lenient(json!({
            "operation":"list","id":"","base":"","dirty_source":"reject"
        }))
        .unwrap();
        assert!(matches!(
            WorktreeOperation::try_from(input).unwrap(),
            WorktreeOperation::List
        ));
    }

    #[test]
    fn creation_requires_a_base_and_explicit_opt_in_to_dirty_sources() {
        for dirty_source in [Value::Null, json!(""), json!("reject")] {
            let input = WorktreeInput::deserialize_lenient(json!({
                "operation":"create","base":"HEAD","dirty_source":dirty_source
            }))
            .unwrap();
            assert!(matches!(
                WorktreeOperation::try_from(input).unwrap(),
                WorktreeOperation::Create {
                    dirty: DirtySource::Reject,
                    ..
                }
            ));
        }
        let input = WorktreeInput::deserialize_lenient(json!({
            "operation":"create","base":"HEAD","dirty_source":"base_only"
        }))
        .unwrap();
        assert!(matches!(
            WorktreeOperation::try_from(input).unwrap(),
            WorktreeOperation::Create {
                dirty: DirtySource::BaseOnly,
                ..
            }
        ));
        for value in [
            json!({"operation":"create"}),
            json!({"operation":"create","base":""}),
            json!({"operation":"create","base":"HEAD","dirty_source":"copy"}),
            json!({"operation":"remove"}),
            json!({"operation":"remove","id":""}),
            json!({"operation":"integrate","id":null}),
        ] {
            assert!(
                WorktreeInput::deserialize_lenient(value.clone())
                    .and_then(WorktreeOperation::try_from)
                    .is_err(),
                "{value}"
            );
        }
    }
}
