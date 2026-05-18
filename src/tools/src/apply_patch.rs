use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait};
use analysis::contexts::context::Context;
use anyhow::anyhow;
use async_trait::async_trait;
use diffy::patch_set::{FileOperation, FilePatch, ParseOptions, PatchKind, PatchSet};
use diffy::Patch;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use turbo_code_macros::{ToolDef, ToolInput};
use utils::files::Files;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ApplyPatch {
    type Input = ApplyPatchInput;
    type Output = ApplyPatchResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        ApplyPatch {
            input,
            id: String::new(),
        }
        .apply_patch()
        .await?;

        Ok(ApplyPatchResult {
            status: "ok".to_string(),
            id: tool_id,
        })
    }

    fn display_input(input: &Self::Input) -> String {
        ApplyPatch {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        ApplyPatch {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.status.clone())
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "apply_patch",
    description = "apply a git patch, supports multiple files, make sure to have the correct format"
)]
pub struct ApplyPatch {
    #[tool(input)]
    pub input: ApplyPatchInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ApplyPatchInput {
    #[tool(description = "The git patch", required)]
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchResult {
    pub status: String,
    pub id: ToolId,
}

impl Display for ApplyPatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let paths: Vec<_> = PatchSet::parse(&self.input.patch, ParseOptions::gitdiff())
            .filter_map(Result::ok)
            .map(|patch| match patch.operation() {
                FileOperation::Delete(path) => format!("delete `{}`", path.as_ref()),
                FileOperation::Create(path) => format!("create `{}`", path.as_ref()),
                FileOperation::Modify { original, modified } => {
                    if original == modified {
                        format!("modify `{}`", modified.as_ref())
                    } else {
                        format!("modify `{}` -> `{}`", original.as_ref(), modified.as_ref())
                    }
                }
                FileOperation::Rename { from, to } => {
                    format!("rename `{}` -> `{}`", from.as_ref(), to.as_ref())
                }
                FileOperation::Copy { from, to } => {
                    format!("copy `{}` -> `{}`", from.as_ref(), to.as_ref())
                }
            })
            .collect();

        match paths.as_slice() {
            [] => write!(f, "- apply patch"),
            [path] => write!(f, "- apply patch: {path}"),
            paths => {
                let shown = paths
                    .iter()
                    .take(3)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                if paths.len() > 3 {
                    write!(f, "- apply patch: {shown}, and {} more", paths.len() - 3)
                } else {
                    write!(f, "- apply patch: {shown}")
                }
            }
        }
    }
}

impl ApplyPatch {
    async fn apply_patch(&self) -> anyhow::Result<()> {
        let patch = PatchSet::parse(&self.input.patch, ParseOptions::gitdiff());
        for patch in patch {
            Self::process_patch(patch?).await?;
        }
        Ok(())
    }

    async fn process_patch<'a>(patch: FilePatch<'a, str>) -> anyhow::Result<()> {
        match patch.operation() {
            FileOperation::Delete(del) => {
                let buf = PathBuf::from_str(&del)?;
                Files::delete_file(&buf).await?;
            }
            FileOperation::Create(create) => {
                let buf = PathBuf::from_str(create)?;
                let data = Self::get_patch(patch)?;
                let content = diffy::apply("", &data)?;
                Files::write_to_file(&buf, &content).await?;
            }
            FileOperation::Modify { original, modified } => {
                let src = PathBuf::from(original.as_ref());
                let dst = PathBuf::from(modified.as_ref());
                let base = Files::read_file(&src).await?;
                let patch = Self::get_patch(patch)?;
                let patched = diffy::apply(&base, &patch)?;
                Files::write_to_file(&dst, &patched).await?;

                if src != dst {
                    Files::delete_file(&src).await?;
                }
            }
            FileOperation::Rename { from, to } => {
                let src = PathBuf::from(from.as_ref());
                let dst = PathBuf::from(to.as_ref());
                Files::rename_file(&src, &dst).await?;
            }
            FileOperation::Copy { from, to } => {
                let src = PathBuf::from(from.as_ref());
                let dst = PathBuf::from(to.as_ref());
                Files::copy_file(&src, &dst).await?;
            }
        }
        Ok(())
    }

    fn get_patch(patch: FilePatch<str>) -> anyhow::Result<Patch<str>> {
        match patch.patch() {
            PatchKind::Text(res) => Ok(res.clone()),
            PatchKind::Binary(_) => Err(anyhow!("Patch can only be Text")),
        }
    }
}
