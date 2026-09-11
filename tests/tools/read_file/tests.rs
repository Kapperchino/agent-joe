fn workspace_scope() -> utils::execution::ExecutionScope {
    utils::execution::ExecutionScope::with_workspace(
        utils::workspace::WorkspacePolicy::workspace(std::env::temp_dir()).unwrap(),
    )
}

use super::*;
use analysis::contexts::context::LineIndexCreator;
use async_trait::async_trait;
use ra_ap_ide::LineIndex;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_FILE_ID: AtomicUsize = AtomicUsize::new(0);

struct TestContext;

struct TestLineIndexCreator;

impl LineIndexCreator for TestLineIndexCreator {
    fn create_index(&self, file_path: &PathBuf) -> anyhow::Result<triomphe::Arc<LineIndex>> {
        let text = std::fs::read_to_string(file_path)?;
        Ok(triomphe::Arc::new(LineIndex::new(&text)))
    }
}

#[async_trait]
impl Context for TestContext {
    type LineIndexCreator = TestLineIndexCreator;

    async fn get_ctx(&self) -> String {
        String::new()
    }

    fn get_root(&self) -> PathBuf {
        PathBuf::new()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(Vec::new())
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        Ok(Box::new(TestLineIndexCreator))
    }

    fn gen_id(&self) -> u64 {
        0
    }
    fn get_id(&self) -> u64 {
        0
    }
}

fn write_temp_file(text: &str) -> PathBuf {
    let id = TEST_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "dumbass-agent-read-range-{}-{id}.txt",
        std::process::id()
    ));
    std::fs::write(&path, text).unwrap();
    path
}

#[tokio::test]
async fn ranges_validate_bounds_and_read_the_current_file_contents() {
    workspace_scope()
        .enter(async {
            let file_path = write_temp_file("original\n");
            for range in [
                Range { start: 0, end: 2 },
                Range { start: 2, end: 2 },
                Range { start: 9, end: 10 },
            ] {
                assert!(
                    ReadFile::read_range(&file_path, range, &TestContext)
                        .await
                        .is_err()
                );
            }
            Files::write_to_file(&file_path, "changed\nnew line\n")
                .await
                .unwrap();
            assert_eq!(
                ReadFile::read_range(&file_path, Range { start: 2, end: 3 }, &TestContext)
                    .await
                    .unwrap(),
                "2: new line"
            );
            Files::write_to_file(&file_path, "").await.unwrap();
            assert!(
                ReadFile::read_range(&file_path, Range { start: 1, end: 2 }, &TestContext)
                    .await
                    .is_err()
            );
            std::fs::remove_file(file_path).unwrap();
        })
        .await;
}

#[tokio::test]
async fn read_range_returns_exclusive_lines_with_utf8_and_line_numbers() {
    workspace_scope()
        .enter(async {
            let file_path = write_temp_file("åéî\nbeta\n終わり\ndelta\n");

            let res = ReadFile::read_range(&file_path, Range { start: 2, end: 4 }, &TestContext)
                .await
                .unwrap();

            assert_eq!(res, "2: beta\n3: 終わり");
            std::fs::remove_file(file_path).unwrap();
        })
        .await;
}

#[tokio::test]
async fn read_range_reads_to_end_when_end_exceeds_file_length() {
    workspace_scope()
        .enter(async {
            let file_path = write_temp_file("one\ntwo\nthree");

            let res = ReadFile::read_range(&file_path, Range { start: 2, end: 99 }, &TestContext)
                .await
                .unwrap();

            assert_eq!(res, "2: two\n3: three");
            std::fs::remove_file(file_path).unwrap();
        })
        .await;
}
