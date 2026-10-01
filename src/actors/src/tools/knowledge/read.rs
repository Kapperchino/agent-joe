use super::*;
use crate::knowledge::RoutedFile;
use common_models::knowledge::LineSpan;
use std::path::Path;
use tools::read_file::{ReadFile, ReadFileInput};
use utils::workspace::Access;

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RelatedContext {
    Ready { file: RoutedFile },
    Unavailable { reason: String },
}

async fn related<C: Context>(
    file_path: &str,
    range: Option<&Range>,
    actor: &ActorContext<C>,
) -> anyhow::Result<RoutedFile> {
    let info = access(actor)?;
    let workspace = info.runtime.scope.workspace()?;
    let path = workspace.relative_path(Path::new(file_path), Access::Read)?;
    let path = SourcePath::try_from(
        path.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )?;
    let budget = budget(&info.services.client, info.runtime.context_budget)?;
    let generation = info.runtime.immutable_workers.knowledge(&info.owner)?;
    generation.check(&workspace, budget).await?;
    let file = generation.file_context(
        &path,
        range.map(|range| LineSpan {
            start: range.start,
            end: range.end,
        }),
    )?;
    generation.check(&workspace, budget).await?;
    Ok(file)
}

pub(super) async fn run<C: Context>(
    file_path: String,
    range: Option<Range>,
    context: &C,
    actor: &ActorContext<C>,
) -> anyhow::Result<Value> {
    utils::execution::ExecutionScope::current()
        .workspace()?
        .check(Path::new(&file_path), Access::Read)?;
    let content = ReadFile {
        input: ReadFileInput {
            file_path: file_path.clone(),
            range: range.clone(),
        },
        id: String::new(),
    }
    .read_file(context)
    .await?;
    let related = match related(&file_path, range.as_ref(), actor).await {
        Ok(file) => RelatedContext::Ready { file },
        Err(error) => RelatedContext::Unavailable {
            reason: error.to_string(),
        },
    };
    Ok(json!({ "action": "read", "file_path": file_path, "content": content, "related": related }))
}
