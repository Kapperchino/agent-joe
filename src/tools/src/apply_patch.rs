use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::fmt::{Display, Formatter};
use std::path::Path;
use turbo_code_macros::{ToolDef, ToolInput};
use utils::diff::{DiffSet, Patch, apply_diff};
use utils::files::Files;
use utils::utils::FnvHashMap;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ApplyPatch {
    type Input = ApplyPatchInput;
    type Output = ApplyPatchResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let paths = DiffSet::new(&input.patch)?
            .patches()
            .iter()
            .flat_map(|patch| match patch {
                Patch::AddFile { path, .. }
                | Patch::DeleteFile { path }
                | Patch::UpdateFile { path, .. } => vec![path.to_path_buf()],
                Patch::MoveFile { from, to, .. } => vec![from.to_path_buf(), to.to_path_buf()],
            })
            .collect::<Vec<_>>();
        cur_context.prepare_edit(&paths).map_err(|error| {
            crate::tool_error::ToolFailure::new(
                crate::tool_error::ToolFailureKind::InvalidInput,
                crate::tool_error::ToolEffects::NotStarted,
                error.to_string(),
            )
        })?;
        let edit = ApplyPatch {
            input,
            id: String::new(),
        }
        .apply_patch()
        .await?;

        cur_context.refresh_workspace().await?;

        Ok(ApplyPatchResult {
            status: "ok".into(),
            edit: utils::changes::EditSummary::from(&edit),
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

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ApplyPatch {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(serde_json::to_string(output)?)
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "apply_patch",
    description = r#"Apply a patch to the workspace.

Use this tool to create, update, move, or delete files. The patch must use the apply-patch format shown below. Paths must be relative to the workspace root. Do not use absolute paths.

Patch format:

*** Begin Patch
*** Add File: path/to/file
+new file content
*** Update File: path/to/file
@@
 context line
-old line
+new line
*** Delete File: path/to/file
*** End Patch

Supported operations:

1. Add a file

*** Begin Patch
*** Add File: src/new_file.rs
+pub fn hello() {
+    println!("hello");
+}
*** End Patch

Every content line in an Add File block must start with `+`.

2. Update a file

*** Begin Patch
*** Update File: src/main.rs
@@
 fn main() {
-    println!("hello");
+    println!("hello, world");
 }
*** End Patch

In update hunks:
- Context lines start with one space.
- Removed lines start with `-`.
- Added lines start with `+`.
- Blank context lines must still start with one space.
- Blank added lines must be written as `+`.
- Blank removed lines must be written as `-`.

3. Delete a file

*** Begin Patch
*** Delete File: src/old_file.rs
*** End Patch

Delete File blocks have no body.

4. Move or rename a file

*** Begin Patch
*** Update File: src/old_name.rs
*** Move to: src/new_name.rs
@@
-pub fn old_name() {}
+pub fn new_name() {}
*** End Patch

Rules:
- Always start with `*** Begin Patch`.
- Always end with `*** End Patch`.
- Use relative paths only.
- Do not include line numbers.
- Do not use Markdown fences inside the patch payload.
- An Update File may contain multiple `@@` hunks.
- A hunk may contain multiple replacement blocks.
- Prefer 2–3 context lines around each change so the patch can be located safely.
- Do not emit a pure insertion hunk without context unless the target location is otherwise unambiguous.
- Do not include unchanged file content unless it is useful context.
- Preserve indentation exactly.
- If a hunk line is unchanged, prefix it with one space.
- If a hunk line is removed, prefix it with `-`.
- If a hunk line is added, prefix it with `+`.

Invalid examples:

Absolute path:

*** Update File: /Users/me/project/src/main.rs

Missing prefix inside hunk:

@@
 fn main() {
println!("bad");
 }

The unchanged line must be:

@@
 fn main() {
 println!("good");
 }

Preferred behavior:
- Make small, focused patches.
- Group related edits in one patch.
- Split unrelated changes into separate patches.
- When editing code, include enough surrounding context to avoid matching the wrong block.
- Never invent files or paths that do not exist unless using Add File."#
)]
pub struct ApplyPatch {
    #[tool(input)]
    pub input: ApplyPatchInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ApplyPatchInput {
    #[tool(description = "The *** Begin Patch formatted diff", required)]
    pub patch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyPatchResult {
    pub status: String,
    pub edit: utils::changes::EditSummary,
    pub id: ToolId,
}

impl Display for ApplyPatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let display = DiffSet::new(&self.input.patch)
            .map(|patch_set| {
                let paths = patch_set
                    .patches()
                    .iter()
                    .map(|patch| match patch {
                        Patch::DeleteFile { path } => format!("delete `{}`", path.display()),
                        Patch::AddFile { path, .. } => format!("create `{}`", path.display()),
                        Patch::UpdateFile { path, .. } => format!("modify `{}`", path.display()),
                        Patch::MoveFile { from, to, .. } => {
                            format!("move `{}` -> `{}`", from.display(), to.display())
                        }
                    })
                    .collect::<Vec<_>>();
                let mut display = match paths.as_slice() {
                    [] => "- apply patch".to_owned(),
                    [path] => format!("- apply patch: {path}"),
                    paths => {
                        let shown = paths
                            .iter()
                            .take(3)
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ");
                        if paths.len() > 3 {
                            format!("- apply patch: {shown}, and {} more", paths.len() - 3)
                        } else {
                            format!("- apply patch: {shown}")
                        }
                    }
                };
                let pretty_diffs = patch_set
                    .into_patches()
                    .into_iter()
                    .filter_map(pretty_patch_diff)
                    .collect::<Vec<_>>();
                if !pretty_diffs.is_empty() {
                    display.push_str("\n\n```diff\n");
                    display.push_str(&pretty_diffs.join("\n"));
                    if !display.ends_with('\n') {
                        display.push('\n');
                    }
                    display.push_str("```");
                }
                display
            })
            .unwrap_or_else(|_| "- apply patch".to_owned());
        f.write_str(&display)
    }
}

fn pretty_patch_diff(patch: Patch<'_>) -> Option<String> {
    let mut output = String::new();
    match patch {
        Patch::AddFile { path, diff } => {
            let content = apply_diff("", Patch::AddFile { path, diff }).ok()?;
            write_content_diff(&mut output, None, Some(path), "", &content).ok()?;
        }
        Patch::DeleteFile { path } => {
            let content = Files::read_file_sync(path).ok()?;
            write_content_diff(&mut output, Some(path), None, &content, "").ok()?;
        }
        Patch::UpdateFile { path, changes } => {
            let content = Files::read_file_sync(path).ok()?;
            let updated = apply_diff(&content, Patch::UpdateFile { path, changes }).ok()?;
            write_content_diff(&mut output, Some(path), Some(path), &content, &updated).ok()?;
        }
        Patch::MoveFile {
            from,
            to,
            changes: None,
        } => {
            Files::read_file_sync(from).ok()?;
            output = format!(
                "diff --git a/{} b/{}\nsimilarity index 100%\nrename from {}\nrename to {}",
                from.display(),
                to.display(),
                from.display(),
                to.display()
            );
        }
        Patch::MoveFile {
            from,
            to,
            changes: Some(changes),
        } => {
            let content = Files::read_file_sync(from).ok()?;
            let updated = apply_diff(
                &content,
                Patch::MoveFile {
                    from,
                    to,
                    changes: Some(changes),
                },
            )
            .ok()?;
            write_content_diff(&mut output, Some(from), Some(to), &content, &updated).ok()?;
        }
    }
    Some(output)
}

fn write_content_diff(
    output: &mut impl std::fmt::Write,
    old_path: Option<&Path>,
    new_path: Option<&Path>,
    old_content: &str,
    new_content: &str,
) -> std::fmt::Result {
    let diff_old_path = old_path.or(new_path).expect("diff path should exist");
    let diff_new_path = new_path.or(old_path).expect("diff path should exist");
    let old_header = diff_header("a", old_path);
    let new_header = diff_header("b", new_path);

    writeln!(
        output,
        "diff --git a/{} b/{}",
        diff_old_path.display(),
        diff_new_path.display()
    )?;

    let diff = TextDiff::from_lines(old_content, new_content);
    write!(
        output,
        "{}",
        diff.unified_diff()
            .header(&old_header, &new_header)
            .context_radius(3)
    )
}

fn diff_header(prefix: &str, path: Option<&Path>) -> String {
    path.map(|path| format!("{prefix}/{}", path.display()))
        .unwrap_or_else(|| "/dev/null".to_string())
}

impl ApplyPatch {
    async fn apply_patch(&self) -> anyhow::Result<utils::changes::EditRecord> {
        let patch = self.input.patch.clone();
        let changes = utils::execution::ExecutionScope::current().changes;
        utils::files::operation(move |workspace| {
            let edits = DiffSet::new(&patch)?
                .into_patches()
                .into_iter()
                .map(|patch| prepared_patch(workspace, patch))
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect();
            changes.apply(workspace, edits)
        })
        .await
    }
}

fn prepared_patch(
    workspace: &utils::workspace::WorkspacePolicy,
    patch: Patch<'_>,
) -> anyhow::Result<Vec<utils::changes::FileEdit>> {
    use utils::changes::{FileEdit, FileVersion};
    match patch {
        Patch::AddFile { path, diff } => {
            let before = workspace.file_version(path)?;
            match before == FileVersion::Missing {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Add destination already exists: {}",
                    path.display()
                )),
            }?;
            let after = before.with_text(apply_diff("", Patch::AddFile { path, diff })?);
            Ok(vec![FileEdit::new(workspace, path, before, after)?])
        }
        Patch::DeleteFile { path } => {
            let before = workspace.file_version(path)?;
            match before != FileVersion::Missing {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Delete target is missing: {}",
                    path.display()
                )),
            }?;
            Ok(vec![FileEdit::new(
                workspace,
                path,
                before,
                FileVersion::Missing,
            )?])
        }
        Patch::UpdateFile { path, changes } => {
            let before = workspace.file_version(path)?;
            let after = before.with_text(apply_diff(
                before.text()?,
                Patch::UpdateFile { path, changes },
            )?);
            Ok(vec![FileEdit::new(workspace, path, before, after)?])
        }
        Patch::MoveFile { from, to, changes } => {
            let before = workspace.file_version(from)?;
            let after = match changes {
                Some(changes) => before.with_text(apply_diff(
                    before.text()?,
                    Patch::MoveFile {
                        from,
                        to,
                        changes: Some(changes),
                    },
                )?),
                None => {
                    before.text()?;
                    before.clone()
                }
            };
            match workspace.relative_path(from, utils::workspace::Access::Write)?
                == workspace.relative_path(to, utils::workspace::Access::Write)?
            {
                true => Ok(vec![FileEdit::new(workspace, from, before, after)?]),
                false => {
                    let destination = workspace.file_version(to)?;
                    match destination == FileVersion::Missing {
                        true => Ok(()),
                        false => Err(anyhow::anyhow!(
                            "Move destination already exists: {}",
                            to.display()
                        )),
                    }?;
                    Ok(vec![
                        FileEdit::new(workspace, to, destination, after)?,
                        FileEdit::new(workspace, from, before, FileVersion::Missing)?,
                    ])
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/tools/apply_patch/tests.rs"]
mod tests;
