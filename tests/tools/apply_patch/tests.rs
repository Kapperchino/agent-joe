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
async fn patch_content_and_destinations_are_preflighted_before_any_write() {
    let root = temp_path("preflight");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("one"), "one\n").unwrap();
    std::fs::write(root.join("two"), "two\n").unwrap();
    let scope = utils::execution::ExecutionScope::with_workspace(
        utils::workspace::WorkspacePolicy::workspace(root.clone()).unwrap(),
    );
    scope.enter(async {
            for second in [
                "*** Update File: two\n@@\n-missing content\n+unexpected",
                "*** Add File: two\n+unexpected",
                "*** Update File: two\n*** Move to: one",
                "*** Delete File: missing",
            ] {
                let patch = tool(format!("*** Begin Patch\n*** Update File: one\n@@\n-one\n+changed\n{second}\n*** End Patch"));
                assert!(patch.apply_patch().await.is_err());
                assert_eq!(std::fs::read_to_string(root.join("one")).unwrap(), "one\n");
                assert_eq!(std::fs::read_to_string(root.join("two")).unwrap(), "two\n");
            }
            Files::read_file(Path::new("one")).await.unwrap();
            std::fs::write(root.join("one"), "user content\n").unwrap();
            let deletion = tool("*** Begin Patch\n*** Delete File: one\n*** End Patch".into());
            let _ = deletion.to_string();
            assert!(deletion.apply_patch().await.is_err());
            assert_eq!(std::fs::read_to_string(root.join("one")).unwrap(), "user content\n");
        }).await;
    std::fs::remove_dir_all(root).unwrap();
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
async fn previews_and_applies_custom_patch_file_operations() {
    workspace_scope()
        .enter(async {
            let added = temp_path("added.txt");
            let updated = temp_path("updated.txt");
            let move_from = temp_path("move-from.txt");
            let move_dir = temp_path("move-dir");
            let move_to = move_dir.join("move-to.txt");
            let deleted = temp_path("deleted.txt");

            tokio::fs::write(&updated, "old\nsame\n").await.unwrap();
            tokio::fs::write(&move_from, "same\n").await.unwrap();
            tokio::fs::write(&deleted, "delete\n").await.unwrap();

            let patch = format!(
                "\
*** Begin Patch
*** Add File: {}
+created
+second line
*** Update File: {}
@@
-old
+changed
 same
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

            let patch = tool(patch);
            let display = patch.to_string();
            assert!(display.contains(&format!("modify `{}`", updated.display())));
            assert!(display.contains("\n\n```diff\n"));
            assert!(display.contains("-old\n+changed\n same"));
            assert!(display.ends_with("```"));
            patch.apply_patch().await.unwrap();

            let added_content = tokio::fs::read_to_string(&added).await.unwrap();
            let updated_content = tokio::fs::read_to_string(&updated).await.unwrap();
            let moved_content = tokio::fs::read_to_string(&move_to).await.unwrap();
            let source_exists = tokio::fs::try_exists(&move_from).await.unwrap();
            let deleted_exists = tokio::fs::try_exists(&deleted).await.unwrap();

            tokio::fs::remove_file(&added).await.unwrap();
            tokio::fs::remove_file(&updated).await.unwrap();
            tokio::fs::remove_file(&move_to).await.unwrap();
            tokio::fs::remove_dir(&move_dir).await.unwrap();

            assert_eq!(added_content, "created\nsecond line");
            assert_eq!(updated_content, "changed\nsame\n");
            assert_eq!(moved_content, "same\n");
            assert!(!source_exists);
            assert!(!deleted_exists);
        })
        .await;
}
