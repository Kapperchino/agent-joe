use crate::utils::Utils;
use anyhow::anyhow;
use grep::regex::RegexMatcher;
use grep::searcher::sinks::UTF8;
use grep::searcher::Searcher;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct TextSearch {}

impl TextSearch {
    pub fn search(text: &str, file_path: &PathBuf) -> anyhow::Result<Vec<(u64, String)>> {
        let matcher = RegexMatcher::new(text)?;
        let mut matches = Vec::new();
        Searcher::new().search_path(
            &matcher,
            file_path,
            UTF8(|lnum, line| {
                matches.push((lnum, line.trim().to_string()));
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
        let matches: HashMap<u64, String> = Self::search(old, file_path)?.into_iter().collect();
        let mut file: Vec<String> = Utils::get_file_content(file_path)
            .await?
            .lines()
            .map(|x| x.to_string())
            .collect();
        matches.into_iter().try_for_each(|(line, _)| {
            let new_line = file.get((line - 1) as usize).map(|l| l.replace(old, new));
            match new_line {
                Some(nl) => {
                    file[(line - 1) as usize] = nl;
                    Ok(())
                }
                None => Err(anyhow!("File is fucked")),
            }
        })?;
        let mut res = file.join("\n");
        res.push_str("\n");
        Utils::write_to_file(file_path, &res).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("turbo_code_test_{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn finds_matching_lines() {
        let path = temp_file("hello world\nfoo bar\nhello rust\n");
        let results = TextSearch::search("hello", &path).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], (1, "hello world".into()));
        assert_eq!(results[1], (3, "hello rust".into()));
    }

    #[tokio::test]
    async fn finds_replace_test() {
        let path = temp_file("hello world\nfoo bar\nhello rust\n");
        TextSearch::search_and_replace("hello", "joe!", &path)
            .await
            .unwrap();
        let file = Utils::get_file_content(&path).await.unwrap();
        let results: Vec<_> = file.lines().collect();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0], "joe! world");
        assert_eq!(results[2], "joe! rust");
    }
}
