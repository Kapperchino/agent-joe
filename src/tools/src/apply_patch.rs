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

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ApplyPatch {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.status.clone())
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
    async fn apply_patch(&self) -> anyhow::Result<()> {
        let patches = DiffSet::new(&self.input.patch)?.into_patches();
        let workspace = utils::execution::ExecutionScope::current().workspace()?;
        for patch in &patches {
            match patch {
                Patch::AddFile { path, .. }
                | Patch::DeleteFile { path }
                | Patch::UpdateFile { path, .. } => {
                    workspace.check(path, utils::workspace::Access::Write)?
                }
                Patch::MoveFile { from, to, .. } => {
                    workspace.check(from, utils::workspace::Access::Write)?;
                    workspace.check(to, utils::workspace::Access::Write)?;
                }
            }
        }
        for patch in patches {
            Self::process_patch(patch).await?;
        }
        Ok(())
    }

    async fn process_patch(patch: Patch<'_>) -> anyhow::Result<()> {
        match patch {
            Patch::DeleteFile { path } => Files::delete_file(path).await,
            Patch::AddFile { path, diff } => {
                let content = apply_diff("", Patch::AddFile { path, diff })?;
                Files::write_to_file(path, &content).await
            }
            Patch::UpdateFile { path, changes } => {
                let content = Files::read_file(path).await?;
                let updated = apply_diff(&content, Patch::UpdateFile { path, changes })?;
                Files::write_to_file(path, &updated).await
            }
            Patch::MoveFile {
                from,
                to,
                changes: None,
            } => Files::rename_file(from, to).await,
            Patch::MoveFile {
                from,
                to,
                changes: Some(changes),
            } => {
                let content = Files::read_file(from).await?;
                let updated = apply_diff(
                    &content,
                    Patch::MoveFile {
                        from,
                        to,
                        changes: Some(changes),
                    },
                )?;
                Files::rename_file(from, to).await?;
                Files::write_to_file(to, &updated).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn workspace_scope() -> utils::execution::ExecutionScope {
        utils::execution::ExecutionScope::with_workspace(
            utils::workspace::WorkspacePolicy::workspace(std::env::temp_dir()).unwrap(),
        )
    }

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

    #[tokio::test]
    async fn previews_and_patch_operations_enforce_the_configured_workspace() {
        let directory = temp_path("policy");
        let root = directory.join("workspace");
        let outside = directory.join("outside");
        std::fs::create_dir_all(root.join(".turbo-code")).unwrap();
        std::fs::write(root.join("file"), "original\n").unwrap();
        std::fs::write(root.join(".turbo-code/config"), "stored credential\n").unwrap();
        std::fs::write(&outside, "outside secret\n").unwrap();
        let scope = utils::execution::ExecutionScope::with_workspace(
            utils::workspace::WorkspacePolicy::workspace(root.clone()).unwrap(),
        );
        scope.enter(async {
            let allowed = tool("*** Begin Patch\n*** Update File: file\n@@\n-original\n+changed\n*** End Patch".into());
            assert!(allowed.to_string().contains("-original\n+changed"));
            allowed.apply_patch().await.unwrap();
            assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "changed\n");
            let protected = tool("*** Begin Patch\n*** Update File: file\n@@\n-changed\n+unexpected\n*** Add File: .git/config\n+unexpected\n*** End Patch".into());
            assert!(protected.apply_patch().await.is_err());
            assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "changed\n");
            let forbidden = tool(format!("*** Begin Patch\n*** Delete File: {}\n*** Delete File: .turbo-code/config\n*** End Patch", outside.display()));
            assert!(!forbidden.to_string().contains("outside secret"));
            assert!(!forbidden.to_string().contains("stored credential"));
            assert!(forbidden.apply_patch().await.is_err());
            let move_outside = tool(format!("*** Begin Patch\n*** Update File: file\n*** Move to: {}\n*** End Patch", outside.display()));
            assert!(move_outside.apply_patch().await.is_err());
            assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "changed\n");
            let move_alias = tool(format!("*** Begin Patch\n*** Update File: file\n*** Move to: {}\n@@\n-changed\n+updated\n*** End Patch", root.join("file").display()));
            move_alias.apply_patch().await.unwrap();
            assert_eq!(std::fs::read_to_string(root.join("file")).unwrap(), "updated\n");
        }).await;
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "outside secret\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".turbo-code/config")).unwrap(),
            "stored credential\n"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn displays_custom_patch_operations() {
        workspace_scope()
            .enter(async {
                let patch = "\
*** Begin Patch
*** Add File: new.txt
+new
*** Update File: old.txt
*** Move to: moved.txt
*** Delete File: gone.txt
*** End Patch";

                let display = tool(patch.to_string()).to_string();

                assert!(display.starts_with(
            "- apply patch: create `new.txt`, move `old.txt` -> `moved.txt`, delete `gone.txt`"
        ));
                assert!(display.contains("\n\n```diff\n"));
                assert!(display.contains("diff --git a/new.txt b/new.txt"));
                assert!(display.contains("--- /dev/null\n+++ b/new.txt"));
                assert!(display.contains("+new"));
            })
            .await;
    }

    #[tokio::test]
    async fn displays_pretty_update_diff() {
        workspace_scope()
            .enter(async {
                let updated = temp_path("display-update.txt");
                std::fs::write(&updated, "old\nsame\n").unwrap();

                let patch = format!(
                    "\
*** Begin Patch
*** Update File: {}
@@
-old
+new
 same
*** End Patch",
                    updated.display(),
                );

                let display = tool(patch).to_string();

                std::fs::remove_file(&updated).unwrap();

                assert!(display.starts_with("- apply patch: modify `"));
                assert!(display.contains("\n\n```diff\n"));
                assert!(display.contains("-old\n+new\n same"));
                assert!(display.ends_with("```"));
            })
            .await;
    }

    #[tokio::test]
    async fn applies_custom_patch_file_operations() {
        workspace_scope()
            .enter(async {
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
            })
            .await;
    }
}
