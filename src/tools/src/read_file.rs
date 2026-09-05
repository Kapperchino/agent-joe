use crate::tool_defs::{Range, ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use turbo_code_macros::{ToolDef, ToolInput};
use utils::{files::Files, utils::FnvHashMap};

mod line_range;
use line_range::LineRange;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for ReadFile {
    type Input = ReadFileInput;
    type Output = ReadFileResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let res = ReadFile {
            input,
            id: String::new(),
        }
        .read_file(cur_context)
        .await?;
        Ok(ReadFileResult { res, id: tool_id })
    }

    fn display_input(input: &Self::Input) -> String {
        ReadFile {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ReadFile {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
    }

    fn effect() -> crate::tool_defs::ToolEffect {
        crate::tool_defs::ToolEffect::Read
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "read_file",
    description = r#"
Read contents from a specific known text file.

The file/symbol index is already available in context. Do NOT use this tool to discover file structure, imports, functions, structs, or symbols.

Prefer a focused line range when the relevant location is known. Omit `range` only for small files or when full-file context is necessary.

Before editing, read the relevant file region unless that exact region is already present in current context.

Prefer parallel read calls.
"#
)]
pub struct ReadFile {
    #[tool(input)]
    pub input: ReadFileInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ReadFileInput {
    #[tool(description = "file path of the file you want to read", required)]
    pub file_path: String,
    #[tool(description = "range of the lines you want to read, empty to read the entire file")]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileResult {
    pub res: String,
    pub id: ToolId,
}

impl Display for ReadFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.input.range {
            Some(range) => write!(
                f,
                "- read `{}` (lines {}-{})",
                self.input.file_path,
                range.start,
                range.end.saturating_sub(1)
            ),
            None => write!(f, "- read `{}`", self.input.file_path),
        }
    }
}

impl ReadFile {
    pub async fn read_file<C: Context>(&self, cur_context: &C) -> anyhow::Result<String> {
        let path = PathBuf::from(&self.input.file_path);
        match Files::is_directory(&path).await? {
            true => Files::get_dir_files(&path).await.map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| entry.name.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            false => match &self.input.range {
                Some(range) => Self::read_range(&path, range.clone(), cur_context).await,
                None => Files::read_file(&path).await.map(|text| {
                    text.lines()
                        .enumerate()
                        .map(|(index, line)| format!("{}: {line}", index + 1))
                        .collect::<Vec<_>>()
                        .join("\n")
                }),
            },
        }
    }

    async fn read_range<C: Context>(path: &PathBuf, range: Range, _: &C) -> anyhow::Result<String> {
        let range = LineRange::try_from(range)?;
        let text = Files::read_file(path).await?;
        range.render(&text)
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
    async fn read_range_returns_requested_exclusive_range_with_line_numbers() {
        workspace_scope()
            .enter(async {
                let file_path = write_temp_file("alpha\nbeta\ngamma\ndelta\n");

                let res =
                    ReadFile::read_range(&file_path, Range { start: 2, end: 4 }, &TestContext)
                        .await
                        .unwrap();

                assert_eq!(res, "2: beta\n3: gamma");
                std::fs::remove_file(file_path).unwrap();
            })
            .await;
    }

    #[tokio::test]
    async fn read_range_reads_to_end_when_end_exceeds_file_length() {
        workspace_scope()
            .enter(async {
                let file_path = write_temp_file("one\ntwo\nthree");

                let res =
                    ReadFile::read_range(&file_path, Range { start: 2, end: 99 }, &TestContext)
                        .await
                        .unwrap();

                assert_eq!(res, "2: two\n3: three");
                std::fs::remove_file(file_path).unwrap();
            })
            .await;
    }

    #[tokio::test]
    async fn read_range_handles_utf8_before_requested_range() {
        workspace_scope()
            .enter(async {
                let file_path = write_temp_file("åéî\nsecond\n終わり\n");

                let res =
                    ReadFile::read_range(&file_path, Range { start: 2, end: 3 }, &TestContext)
                        .await
                        .unwrap();

                assert_eq!(res, "2: second");
                std::fs::remove_file(file_path).unwrap();
            })
            .await;
    }
}
