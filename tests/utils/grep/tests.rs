#[test]
fn search_limits_reject_unbounded_context_and_aggregate_output() {
    assert!(super::SearchContext::new(usize::MAX, 1).is_err());
    let results = super::SearchResults {
        groups: vec![],
        bytes: 32 * 1024 * 1024,
    };
    assert!(
        results
            .append(vec![super::GrepMatch {
                path: "source".into(),
                lines: vec![]
            }])
            .is_err()
    );
    let mut collector = super::LineCollector {
        groups: vec![],
        bytes: 32 * 1024 * 1024,
    };
    assert!(collector.push(Some(1), b"match").is_err());
}

fn workspace_scope() -> crate::execution::ExecutionScope {
    crate::execution::ExecutionScope::with_workspace(
        crate::workspace::WorkspacePolicy::workspace(std::env::temp_dir()).unwrap(),
    )
}

use super::*;
use std::path::PathBuf;

fn temp_file(content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("grep_test_{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, content).unwrap();
    path
}

#[tokio::test]
async fn grep_splits_disjoint_match_groups_per_file() {
    workspace_scope()
        .enter(async {
            let path = temp_file("alpha\nmatch here\ncharlie\n\nomega\nmatch\nzulu\n");

            let results = Grep::grep("match", vec![path.clone()], 1, 1).await.unwrap();

            assert_eq!(results.len(), 2);
            assert!(
                results
                    .iter()
                    .all(|group| group.path == path.to_string_lossy())
            );
            assert_eq!(
                results[0].lines,
                vec![
                    GrepLine {
                        line_number: Some(1),
                        line: "alpha".into(),
                    },
                    GrepLine {
                        line_number: Some(2),
                        line: "match here".into(),
                    },
                    GrepLine {
                        line_number: Some(3),
                        line: "charlie".into(),
                    },
                ]
            );
            assert_eq!(
                results[1].lines,
                vec![
                    GrepLine {
                        line_number: Some(5),
                        line: "omega".into(),
                    },
                    GrepLine {
                        line_number: Some(6),
                        line: "match".into(),
                    },
                    GrepLine {
                        line_number: Some(7),
                        line: "zulu".into(),
                    },
                ]
            );
            std::fs::remove_file(path).unwrap();
        })
        .await;
}
