use crate::tool_defs::{Range, ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::{Context, LineIndexCreator};
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::fmt::Write;
use std::fmt::{Display, Formatter};
use std::io::SeekFrom;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_stream::wrappers::LinesStream;
use turbo_code_macros::{ToolDef, ToolInput};

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

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        ReadFile {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(output.res.clone())
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

Files, paths, and symbols may already be available in context. Do not use this tool for discovery, directory listing, or symbol search. Use it only to retrieve the actual text of a known file when existing context is insufficient.

Prefer a focused line range when the relevant location is known. Omit `range` only for small files or when full-file context is necessary.

Before editing, read the relevant file region unless that exact region is already present in current context.
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
        let file_path: PathBuf = self.input.file_path.clone().into();
        let root = cur_context.get_root();
        let file_path = if file_path.starts_with(&root) {
            file_path.strip_prefix(root).map(|t| t.to_path_buf())
        } else {
            Ok(file_path)
        }?;
        match file_path.is_dir() {
            false => match &self.input.range {
                None => Self::read_entire_file(&file_path).await,
                Some(range) => Self::read_range(&file_path, range.clone(), cur_context).await,
            },
            true => Self::read_dir(&file_path).await,
        }
    }

    // one day we will have good async streams
    async fn read_dir(file_path: &PathBuf) -> anyhow::Result<String> {
        let mut entries = tokio::fs::read_dir(file_path).await?;
        let mut result = String::new();
        while let Some(entry) = entries.next_entry().await? {
            result.push_str(&entry.file_name().to_string_lossy());
            result.push('\n');
        }
        Ok(result)
    }

    async fn read_entire_file(file_path: &PathBuf) -> anyhow::Result<String> {
        let file = File::open(file_path).await?;
        let reader = BufReader::new(file);

        let lines = LinesStream::new(reader.lines());

        let res = lines
            .enumerate()
            .map(|(line, res)| res.map(|l_content| format!("{line}: {l_content}")))
            .try_fold(String::new(), |acc, line| async move {
                if acc.is_empty() {
                    Ok(format!("{line}"))
                } else {
                    Ok(format!("{acc}\n{line}"))
                }
            })
            .await?;

        Ok(res)
    }
    async fn read_range<C: Context>(
        file_path: &PathBuf,
        range: Range,
        cur_context: &C,
    ) -> anyhow::Result<String> {
        let line_index = cur_context.line_index_creator().await?;
        let line_index = line_index.create_index(file_path)?;

        let start: u32 = line_index
            .line(range.start.saturating_sub(1))
            .unwrap()
            .start()
            .into();
        let start: u64 = start as u64;
        let end: u64 = line_index
            .line(range.end.saturating_sub(1))
            .map(|l| l.start().into())
            .unwrap_or(u32::MAX) as u64;
        let start_line = range.start.saturating_sub(1);
        let mut file = File::open(file_path).await?;
        file.seek(SeekFrom::Start(start as u64)).await?;
        let file_size = file.metadata().await?.len();
        let remaining: usize = (file_size - start) as usize;
        let buf_size = min(remaining, (end - start) as usize);
        let mut buf = vec![0; buf_size];
        file.read_exact(&mut buf).await?;
        let text = std::str::from_utf8(&buf)?;
        let mut res = String::with_capacity(text.len() + 128);
        text.lines().enumerate().try_for_each(|(i, line)| {
            if i > 0 {
                res.push('\n');
            }
            let line_no = start_line + i as u32 + 1;
            write!(&mut res, "{line_no}: {line}")
        })?;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn read_range_returns_requested_exclusive_range_with_line_numbers() {
        let file_path = write_temp_file("alpha\nbeta\ngamma\ndelta\n");

        let res = ReadFile::read_range(&file_path, Range { start: 2, end: 4 }, &TestContext)
            .await
            .unwrap();

        assert_eq!(res, "2: beta\n3: gamma");
        std::fs::remove_file(file_path).unwrap();
    }

    #[tokio::test]
    async fn read_range_reads_to_end_when_end_exceeds_file_length() {
        let file_path = write_temp_file("one\ntwo\nthree");

        let res = ReadFile::read_range(&file_path, Range { start: 2, end: 99 }, &TestContext)
            .await
            .unwrap();

        assert_eq!(res, "2: two\n3: three");
        std::fs::remove_file(file_path).unwrap();
    }

    #[tokio::test]
    async fn read_range_handles_utf8_before_requested_range() {
        let file_path = write_temp_file("åéî\nsecond\n終わり\n");

        let res = ReadFile::read_range(&file_path, Range { start: 2, end: 3 }, &TestContext)
            .await
            .unwrap();

        assert_eq!(res, "2: second");
        std::fs::remove_file(file_path).unwrap();
    }
}
