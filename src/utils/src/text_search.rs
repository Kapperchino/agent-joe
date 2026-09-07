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
}
