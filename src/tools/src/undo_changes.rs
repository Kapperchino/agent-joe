use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "undo_changes",
    description = "Undo a fully applied Joe edit by its recorded ID from apply_patch or review_changes. Every affected file must still match Joe's recorded result, including permissions and absence. Any conflict rejects the whole preflight; partial filesystem failures remain journaled. The Git index is preserved."
)]
pub struct UndoChanges {
    #[tool(input)]
    pub input: UndoChangesInput,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct UndoChangesInput {
    #[tool(description = "Recorded Joe edit ID to undo", required)]
    pub edit_id: String,
}

impl Display for UndoChanges {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "- undo Joe edit {}", self.input.edit_id)
    }
}

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for UndoChanges {
    type Input = UndoChangesInput;
    type Output = utils::changes::EditSummary;
    async fn run(
        input: Self::Input,
        _: ToolId,
        context: &C,
        _: &A,
    ) -> anyhow::Result<Self::Output> {
        let changes = utils::execution::ExecutionScope::current().changes;
        let snapshot = changes.snapshot()?;
        let record = snapshot
            .records
            .iter()
            .find(|record| record.id == input.edit_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown Joe edit ID"))?;
        context.prepare_edit(
            &record
                .edits
                .iter()
                .map(|edit| edit.path.clone())
                .collect::<Vec<_>>(),
        )?;
        let result =
            utils::files::operation(move |workspace| changes.undo(workspace, &input.edit_id))
                .await?;
        context.refresh_workspace().await?;
        Ok(utils::changes::EditSummary::from(&result))
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
}
