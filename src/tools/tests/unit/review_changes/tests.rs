use super::*;
use utils::changes::{ChangeOwnership, Review, ReviewedFile};

#[test]
fn review_keeps_distinct_evidence_and_renders_identical_diffs_once() {
    let diff = "--- a/file\n+++ b/file\n@@ -1 +1 @@\n-before\n+after\n".repeat(200);
    let review = Review {
        index_changes: vec![],
        baseline_git: None,
        current_git: None,
        baseline_staged: "baseline staged change\n".into(),
        staged: "baseline staged change\n".into(),
        unstaged: "concurrent unstaged change\n".into(),
        changes: vec![ReviewedFile {
            path: "file".into(),
            ownership: ChangeOwnership::JoeAndExternal,
            task_diff: diff.clone(),
            joe_diff: diff.clone(),
            current_fingerprint: "fingerprint".into(),
        }],
        edits: vec![],
    };
    let rendered = serde_json::to_string(&ReviewContent::new(&review)).unwrap();
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["changes"][0]["path"], "file");
    assert_eq!(value["changes"][0]["task_diff"], diff);
    assert_eq!(
        value["changes"][0]["joe_diff"]["same_as"],
        "/changes/0/task_diff"
    );
    assert_eq!(value["staged"]["same_as"], "/baseline_staged");
    for pointer in ["/changes/0/joe_diff", "/staged"] {
        let reference = value.pointer(pointer).unwrap()["same_as"].as_str().unwrap();
        let original = serde_json::to_value(&review).unwrap();
        assert_eq!(value.pointer(reference), original.pointer(pointer));
    }
    assert!(rendered.contains("concurrent unstaged change"));
    assert!(rendered.contains("joe_and_external"));
    assert!(rendered.contains("fingerprint"));
    assert!(rendered.len() < serde_json::to_string(&review).unwrap().len() * 2 / 3);
    let changed = Review {
        changes: vec![ReviewedFile {
            joe_diff: "separate Joe edit\n".into(),
            ..review.changes[0].clone()
        }],
        ..review
    };
    let rendered = serde_json::to_value(ReviewContent::new(&changed)).unwrap();
    assert_eq!(rendered["changes"][0]["task_diff"], diff);
    assert_eq!(rendered["changes"][0]["joe_diff"], "separate Joe edit\n");
}
