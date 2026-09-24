use super::*;
use utils::changes::{ChangeOwnership, Review, ReviewedFile};

#[test]
fn review_accepts_an_optional_validated_commit_subject() {
    for value in [
        serde_json::json!({}),
        serde_json::json!({ "commit_message": null }),
    ] {
        let input: ReviewChangesInput = serde_json::from_value(value).unwrap();
        assert!(input.commit_message().unwrap().is_none());
    }
    for subject in [
        "",
        " \n ",
        "Fix the value\nMore details",
        "`Fix the value`",
        &"x".repeat(73),
    ] {
        let input = ReviewChangesInput {
            commit_message: Some(subject.into()),
        };
        assert!(input.commit_message().is_err());
    }
    let input = ReviewChangesInput {
        commit_message: Some("  Fix the return value  ".into()),
    };
    assert_eq!(
        input.commit_message().unwrap().unwrap().as_str(),
        "Fix the return value"
    );
    let input = ReviewChangesInput {
        commit_message: Some("x".repeat(72)),
    };
    assert!(input.commit_message().is_ok());
}

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
