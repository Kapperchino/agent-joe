use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::diff::{apply_diff, DiffSet, Patch};
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
    pub id: ToolId,
}

impl Display for ApplyPatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let paths: Vec<_> = DiffSet::new(&self.input.patch)
            .map(|patches| {
                patches
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
                    .collect()
            })
            .unwrap_or_default();

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
        let patches = DiffSet::new(&self.input.patch)?;
        for patch in patches.into_patches() {
            Self::process_patch(patch).await?;
        }
        Ok(())
    }

    async fn process_patch(patch: Patch<'_>) -> anyhow::Result<()> {
        match patch {
            Patch::DeleteFile { path } => {
                Files::delete_file(&path.to_path_buf()).await?;
            }
            Patch::AddFile { path, diff } => {
                let content = apply_diff("", Patch::AddFile { path, diff })?;
                Files::write_to_file(&path.to_path_buf(), &content).await?;
            }
            Patch::UpdateFile { path, changes } => {
                let path = path.to_path_buf();
                let base = Files::read_file(&path).await?;
                let patched = apply_diff(
                    &base,
                    Patch::UpdateFile {
                        path: &path,
                        changes,
                    },
                )?;
                Files::write_to_file(&path, &patched).await?;
            }
            Patch::MoveFile {
                from,
                to,
                changes: None,
            } => {
                Files::rename_file(&from.to_path_buf(), &to.to_path_buf()).await?;
            }
            Patch::MoveFile {
                from,
                to,
                changes: Some(changes),
            } => {
                let src = from.to_path_buf();
                let dst = to.to_path_buf();
                let base = Files::read_file(&src).await?;
                let patched = apply_diff(
                    &base,
                    Patch::MoveFile {
                        from: &src,
                        to: &dst,
                        changes: Some(changes),
                    },
                )?;
                if let Some(parent) = dst.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                Files::write_to_file(&dst, &patched).await?;
                if src != dst {
                    Files::delete_file(&src).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "dumbass-agent-apply-patch-{}-{nonce}-{name}",
            std::process::id()
        ))
    }

    fn tool(patch: String) -> ApplyPatch {
        ApplyPatch {
            input: ApplyPatchInput { patch },
            id: String::new(),
        }
    }

    #[test]
    fn displays_custom_patch_operations() {
        let patch = "\
*** Begin Patch
*** Add File: new.txt
+new
*** Update File: old.txt
*** Move to: moved.txt
*** Delete File: gone.txt
*** End Patch";

        assert_eq!(
            tool(patch.to_string()).to_string(),
            "- apply patch: create `new.txt`, move `old.txt` -> `moved.txt`, delete `gone.txt`"
        );
    }

    #[tokio::test]
    async fn applies_custom_patch_file_operations() {
        let added = temp_path("added.txt");
        let updated = temp_path("updated.txt");
        let move_from = temp_path("move-from.txt");
        let move_dir = temp_path("move-dir");
        let move_to = move_dir.join("move-to.txt");
        let deleted = temp_path("deleted.txt");

        tokio::fs::write(&updated, "old\n").await.unwrap();
        tokio::fs::write(&move_from, "same\n").await.unwrap();
        tokio::fs::write(&deleted, "delete\n").await.unwrap();

        let patch = format!(
            "\
*** Begin Patch
*** Add File: {}
+created
*** Update File: {}
@@
-old
+changed
*** Update File: {}
*** Move to: {}
*** Delete File: {}
*** End Patch",
            added.display(),
            updated.display(),
            move_from.display(),
            move_to.display(),
            deleted.display(),
        );

        tool(patch).apply_patch().await.unwrap();

        let added_content = tokio::fs::read_to_string(&added).await.unwrap();
        let updated_content = tokio::fs::read_to_string(&updated).await.unwrap();
        let moved_content = tokio::fs::read_to_string(&move_to).await.unwrap();
        let source_exists = tokio::fs::try_exists(&move_from).await.unwrap();
        let deleted_exists = tokio::fs::try_exists(&deleted).await.unwrap();

        tokio::fs::remove_file(&added).await.unwrap();
        tokio::fs::remove_file(&updated).await.unwrap();
        tokio::fs::remove_file(&move_to).await.unwrap();
        tokio::fs::remove_dir(&move_dir).await.unwrap();

        assert_eq!(added_content, "created");
        assert_eq!(updated_content, "changed\n");
        assert_eq!(moved_content, "same\n");
        assert!(!source_exists);
        assert!(!deleted_exists);
    }
}
