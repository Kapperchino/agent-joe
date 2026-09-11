fn workspace_scope() -> crate::execution::ExecutionScope {
    crate::execution::ExecutionScope::with_workspace(
        crate::workspace::WorkspacePolicy::workspace(std::env::temp_dir()).unwrap(),
    )
}

use super::*;

fn temp_file(content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("turbo_code_test_{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, content).unwrap();
    path
}

#[tokio::test]
async fn finds_matching_lines() {
    workspace_scope()
        .enter(async {
            let path = temp_file("hello world\nfoo bar\nhello rust\n");
            let results = TextSearch::search_str("hello", &path).unwrap();
            assert_eq!(results.len(), 2);
            assert_eq!(
                results[0],
                TextMatch {
                    line: 1,
                    content: "hello world".into()
                }
            );
            assert_eq!(
                results[1],
                TextMatch {
                    line: 3,
                    content: "hello rust".into()
                }
            );
        })
        .await;
}

#[tokio::test]
async fn replaces_all_matches_and_preserves_other_lines() {
    workspace_scope()
        .enter(async {
            let path = temp_file("hello world\nfoo bar\nhello rust\n");
            TextSearch::search_and_replace("hello", "joe!", &path)
                .await
                .unwrap();
            let file = Files::read_file(&path).await.unwrap();
            assert_eq!(file, "joe! world\nfoo bar\njoe! rust\n");
            std::fs::remove_file(path).unwrap();
        })
        .await;
}
