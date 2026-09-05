use crate::files::Files;
use grep::regex::RegexMatcherBuilder;
use grep::searcher::SearcherBuilder;
use grep::searcher::sinks::UTF8;
use std::path::PathBuf;

pub struct TextSearch {}

#[derive(Debug, PartialEq, Eq)]
pub struct TextMatch {
    pub line: u64,
    pub content: String,
}

impl TextSearch {
    pub fn search_str(text: &str, file_path: &PathBuf) -> anyhow::Result<Vec<TextMatch>> {
        let content = Files::read_file_sync(file_path)?;
        let matcher = RegexMatcherBuilder::new().fixed_strings(true).build(text)?;
        let mut matches = Vec::new();
        SearcherBuilder::new()
            .multi_line(text.contains('\n'))
            .build()
            .search_slice(
                &matcher,
                content.as_bytes(),
                UTF8(|line, text| {
                    matches.push(TextMatch {
                        line,
                        content: text.trim().to_owned(),
                    });
                    Ok(true)
                }),
            )?;
        Ok(matches)
    }

    pub async fn search_and_replace(
        old: &str,
        new: &str,
        file_path: &PathBuf,
    ) -> anyhow::Result<()> {
        let content = Files::read_file(file_path).await?;
        let replaced = content.replace(old, new);
        if content == replaced {
            Err(anyhow::anyhow!("Pattern not found in file"))
        } else {
            Files::write_to_file(file_path, &replaced).await
        }
    }
}

#[cfg(test)]
mod tests {
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
    async fn finds_replace_test() {
        workspace_scope()
            .enter(async {
                let path = temp_file("hello world\nfoo bar\nhello rust\n");
                TextSearch::search_and_replace("hello", "joe!", &path)
                    .await
                    .unwrap();
                let file = Files::read_file(&path).await.unwrap();
                let results: Vec<_> = file.lines().collect();
                assert_eq!(results.len(), 3);
                assert_eq!(results[0], "joe! world");
                assert_eq!(results[2], "joe! rust");
            })
            .await;
    }

    #[tokio::test]
    async fn finds_replace_test_whole() {
        workspace_scope()
            .enter(async {
                let string = r#"use futures::future;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::wrappers::ReadDirStream;

pub struct Utils {}
impl Utils {
    pub async fn get_dir_files(dir: &PathBuf) -> anyhow::Result<Vec<DirEntry>> {
        use tokio_stream::StreamExt;

        let read_dir = fs::read_dir(dir).await?;
        let read_dir_stream = ReadDirStream::new(read_dir);
        let res = read_dir_stream
            .fold(vec![], |mut acc, item| {
                match item {
                    Ok(entry) => {
                        acc.push(entry);
                    }
                    Err(_) => {
                        println!("error with getting files")
                    }
                };
                acc
            })
            .await;
        Ok(res)
    }

    pub async fn get_file_content(dir: &PathBuf) -> anyhow::Result<String> {
        let str = fs::read_to_string(dir).await?;
        Ok(str)
    }

    pub async fn write_to_file(dir: &PathBuf, content: &str) -> anyhow::Result<()> {
        fs::write(dir, content).await?;
        Ok(())
    }

    pub async fn get_files_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        future::join_all(paths.into_iter().map(async |file| {
            Utils::get_file_content(&file)
                .await
                .map(|content| (file, content))
        }))
        .await
        .into_iter()
        .collect()
    }

    pub async fn get_file_hashes_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        use futures::StreamExt;

        let contents = Self::get_files_for_paths(paths).await?;
        let results: Vec<_> = futures::stream::iter(contents)
            .map(|(path, content)| {
                tokio::spawn(
                    async move { (path, blake3::hash(content.as_bytes()).as_bytes().to_vec()) },
                )
            })
            .buffer_unordered(100)
            .collect::<Vec<_>>()
            .await;

        let res = results.into_iter().collect::<Result<Vec<_>, _>>()?;
        Ok(res)
    }
}
"#;
                let new_str = string.replace("res", "deez");
                let path = temp_file(string);
                TextSearch::search_and_replace(string, &new_str, &path)
                    .await
                    .unwrap();
                let file = Files::read_file(&path).await.unwrap();
                assert_eq!(file, new_str);
            })
            .await;
    }
}
