use futures::future;
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
