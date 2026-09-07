use crate::{
    apply_patch::{ApplyPatch, ApplyPatchInput},
    read_file::{ReadFile, ReadFileInput},
    tool_defs::{LenientDeserialize, Range, ToolId, ToolTrait},
    tool_error::{ToolEffects, ToolFailure, ToolFailureKind},
};
use analysis::contexts::{
    context::{Context, LineIndexCreator},
    rust_context::RustContext,
    rust_empty_context::RustEmptyContext,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use utils::{execution::ExecutionScope, files::Files, workspace::WorkspacePolicy};

struct Fixture {
    root: PathBuf,
    scope: ExecutionScope,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "joe-discovery-tools-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let policy = WorkspacePolicy::workspace(root).unwrap();
        Self {
            root: policy.root().to_path_buf(),
            scope: ExecutionScope::with_workspace(policy),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn id() -> ToolId {
    ToolId {
        id: "tool".to_owned().try_into().unwrap(),
        call_id: None,
    }
}

async fn exercise<C: Context>(context: &C) {
    context.effective_instructions().unwrap();
    let input = ApplyPatchInput {
        patch: "*** Begin Patch\n*** Add File: docs/new.md\n+αβ\n+second\n*** End Patch".into(),
    };
    let error = <ApplyPatch as ToolTrait<C, ()>>::run(input.clone(), id(), context, &())
        .await
        .unwrap_err();
    let failure = error.downcast_ref::<ToolFailure>().unwrap();
    assert_eq!(failure.kind, ToolFailureKind::InvalidInput);
    assert_eq!(failure.effects, ToolEffects::NotStarted);
    assert!(!context.get_root().join("docs/new.md").exists());
    let instructions = context.effective_instructions().unwrap();
    assert!(instructions.contains("Markdown rule"));
    <ApplyPatch as ToolTrait<C, ()>>::run(input, id(), context, &())
        .await
        .unwrap();
    let reader = ReadFile {
        input: ReadFileInput {
            file_path: "docs/new.md".into(),
            range: Some(Range { start: 1, end: 2 }),
        },
        id: String::new(),
    };
    assert_eq!(reader.read_file(context).await.unwrap(), "1: αβ");
    let index = context.line_index_creator().await.unwrap();
    assert_eq!(
        index
            .create_index(&PathBuf::from("docs/new.md"))
            .unwrap()
            .line_col(5.into())
            .line,
        1
    );
    Files::write_to_file(&PathBuf::from("docs/new.md"), "changed\n終わり\nthird\n")
        .await
        .unwrap();
    assert_eq!(reader.read_file(context).await.unwrap(), "1: changed");
    assert_eq!(
        index
            .create_index(&PathBuf::from("docs/new.md"))
            .unwrap()
            .line_col(8.into())
            .line,
        1
    );
    assert!(
        context
            .get_files()
            .await
            .unwrap()
            .contains(&PathBuf::from("docs/new.md"))
    );
    Files::write_to_file(&PathBuf::from("docs/AGENTS.md"), "Updated Markdown rule")
        .await
        .unwrap();
    let rename = ApplyPatchInput { patch: "*** Begin Patch\n*** Update File: docs/new.md\n*** Move to: elsewhere/new.md\n*** End Patch".into() };
    assert!(
        <ApplyPatch as ToolTrait<C, ()>>::run(rename.clone(), id(), context, &())
            .await
            .is_err()
    );
    assert!(
        context
            .effective_instructions()
            .unwrap()
            .contains("Destination rule")
    );
    <ApplyPatch as ToolTrait<C, ()>>::run(rename, id(), context, &())
        .await
        .unwrap();
    assert!(
        !context
            .get_files()
            .await
            .unwrap()
            .contains(&PathBuf::from("docs/new.md"))
    );
    let files = context.get_files().await.unwrap();
    assert!(files.contains(&PathBuf::from("elsewhere/new.md")));
    let empty = ReadFile {
        input: ReadFileInput {
            file_path: "empty.txt".into(),
            range: None,
        },
        id: String::new(),
    };
    assert_eq!(empty.read_file(context).await.unwrap(), "");
    let invalid = ReadFile {
        input: ReadFileInput {
            range: Some(Range { start: 1, end: 2 }),
            ..empty.input
        },
        id: String::new(),
    };
    let error = invalid.read_file(context).await.unwrap_err();
    let failure = error.downcast_ref::<ToolFailure>().unwrap();
    assert_eq!(failure.kind, ToolFailureKind::InvalidInput);
    let detail: serde_json::Value = serde_json::from_str(&failure.message).unwrap();
    assert_eq!(detail["code"], "start_beyond_eof");
    assert_eq!(detail["line_count"], 0);
}

#[tokio::test]
async fn new_non_rust_files_and_scoped_edits_work_in_simple_and_worker_contexts() {
    for worker in [false, true] {
        let fixture = Fixture::new();
        fixture
            .scope
            .enter(async {
                Files::write_to_file(&PathBuf::from("docs/AGENTS.md"), "Markdown rule")
                    .await
                    .unwrap();
                Files::write_to_file(&PathBuf::from("elsewhere/AGENTS.md"), "Destination rule")
                    .await
                    .unwrap();
                Files::write_to_file(&PathBuf::from("empty.txt"), "")
                    .await
                    .unwrap();
                let context = RustContext::new("Operating policy".into(), 0, fixture.root.clone())
                    .await
                    .unwrap();
                match worker {
                    false => exercise(&context).await,
                    true => exercise(&RustEmptyContext::new(context, 1)).await,
                }
            })
            .await;
    }
}

#[test]
fn malformed_optional_search_filters_and_ranges_are_rejected() {
    assert!(
        ReadFileInput::deserialize_lenient(
            serde_json::json!({"file_path": "x", "range": {"start": -1, "end": 2}})
        )
        .is_err()
    );
    assert!(
        crate::grep::GrepInput::deserialize_lenient(
            serde_json::json!({"regex": "x", "add_start": 0, "add_end": 0, "exclude": ["secret"]})
        )
        .is_err()
    );
    assert!(
        crate::find_files::FindFilesInput::deserialize_lenient(
            serde_json::json!({"pattern": "x", "limit": "unbounded"})
        )
        .is_err()
    );
}

#[tokio::test]
async fn semantic_ranges_refresh_after_modify_rename_and_delete_without_watcher_events() {
    let fixture = Fixture::new();
    fixture
        .scope
        .enter(async {
            Files::write_to_file(&PathBuf::from("src/lib.rs"), "pub fn initial() {}\n")
                .await
                .unwrap();
            let context = RustContext::new("Policy".into(), 0, fixture.root.clone())
                .await
                .unwrap();
            let initial = context.get_proj_meta().await.unwrap();
            assert_eq!(initial.functions[0].full_range.start, 1);
            assert_eq!(initial.functions[0].full_range.end, 2);
            Files::write_to_file(&PathBuf::from("src/lib.rs"), "\npub fn changed() {\n}\n")
                .await
                .unwrap();
            let changed = context.get_proj_meta().await.unwrap();
            assert_eq!(changed.functions[0].name, "changed");
            assert_eq!(changed.functions[0].full_range.start, 2);
            assert_eq!(changed.functions[0].full_range.end, 4);
            Files::rename_file(&PathBuf::from("src/lib.rs"), &PathBuf::from("src/new.rs"))
                .await
                .unwrap();
            let moved = context.get_proj_meta().await.unwrap();
            assert!(
                moved
                    .functions
                    .iter()
                    .all(|function| function.rpath.inner == "src/new.rs")
            );
            Files::delete_file(&PathBuf::from("src/new.rs"))
                .await
                .unwrap();
            assert!(context.get_proj_meta().await.unwrap().functions.is_empty());
        })
        .await;
}

#[tokio::test]
async fn insertion_uses_one_based_after_semantics_and_rejects_invalid_lines() {
    use crate::insert_after_line::{InsertAfterLine, InsertAfterLineInput};
    let fixture = Fixture::new();
    fixture
        .scope
        .enter(async {
            Files::write_to_file(&PathBuf::from("notes.md"), "first\nlast\n")
                .await
                .unwrap();
            let context = RustContext::new("Policy".into(), 0, fixture.root.clone())
                .await
                .unwrap();
            <InsertAfterLine as ToolTrait<RustContext, ()>>::run(
                InsertAfterLineInput {
                    content: "middle".into(),
                    file_path: "notes.md".into(),
                    line_num: 1,
                },
                id(),
                &context,
                &(),
            )
            .await
            .unwrap();
            assert_eq!(
                Files::read_file(&PathBuf::from("notes.md")).await.unwrap(),
                "first\nmiddle\nlast\n"
            );
            for line_num in [0, 4, usize::MAX] {
                assert!(
                    <InsertAfterLine as ToolTrait<RustContext, ()>>::run(
                        InsertAfterLineInput {
                            content: "bad".into(),
                            file_path: "notes.md".into(),
                            line_num
                        },
                        id(),
                        &context,
                        &()
                    )
                    .await
                    .is_err()
                );
            }
            assert_eq!(
                Files::read_file(&PathBuf::from("notes.md")).await.unwrap(),
                "first\nmiddle\nlast\n"
            );
        })
        .await;
}
