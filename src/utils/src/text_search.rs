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
#[path = "../../../tests/utils/text_search/tests.rs"]
mod tests;
