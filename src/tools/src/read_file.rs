use crate::tool_defs::ToolInputSchema;
use crate::tool_defs::{Range, ToolDefTrait, ToolId, ToolTrait};
use analysis::contexts::context::{Context, LineIndexCreator};
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::fmt::{Display, Formatter};
use std::io::SeekFrom;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio_stream::wrappers::LinesStream;
use turbo_code_macros::{ToolDef, ToolInput};

#[async_trait]
impl ToolTrait for ReadFile {
    type Input = ReadFileInput;
    type Output = ReadFileResult;

    async fn run<C: Context>(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
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
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "read_file",
    description = "Reads a file at file_path or a section of it defined by range or the files in a director if a dir path is provided,\
     do not read the entire file unless you need to"
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
        let end: u32 = line_index
            .line(range.end.saturating_sub(1))
            .map(|l| l.end().into())
            .unwrap_or(u32::MAX);
        let start_line = range.start.saturating_sub(1);
        let mut file = File::open(file_path).await?;
        file.seek(SeekFrom::Start(start as u64)).await?;
        let file_size = file.metadata().await?.len();
        let remaining: usize = (file_size - file.stream_position().await?) as usize;
        let buf_size = min(remaining, (end - start) as usize);
        let mut buf = vec![0; buf_size];
        file.read_exact(&mut buf).await?;
        let res = String::from_utf8(buf)?;
        let res = res
            .lines()
            .enumerate()
            .fold(String::new(), |acc, (i, line)| {
                let i = start_line + i as u32 + 1;
                if acc.is_empty() {
                    format!("{i}: {line}")
                } else {
                    format!("{acc}\n{i}: {line}")
                }
            });
        Ok(res)
    }
}
