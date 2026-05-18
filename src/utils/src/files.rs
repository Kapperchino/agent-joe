use futures::future;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::{DirEntry, File};
use tokio_stream::wrappers::ReadDirStream;

pub struct Files {}

impl Files {
    pub async fn delete_file(file: &PathBuf) -> anyhow::Result<()> {
        fs::remove_file(file).await?;
        Ok(())
    }

    pub async fn create_file(file: &PathBuf, data: &str) -> anyhow::Result<()> {
        let _ = File::create(file).await?;
        Ok(())
    }

    pub async fn read_file(file: &PathBuf) -> anyhow::Result<String> {
        let res = fs::read_to_string(file).await?;
        Ok(res)
    }

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

    pub async fn write_to_file(dir: &PathBuf, content: &str) -> anyhow::Result<()> {
        fs::write(dir, content).await?;
        Ok(())
    }

    pub async fn rename_file(from: &PathBuf, to: &PathBuf) -> anyhow::Result<()> {
        Self::create_parent_dirs(to).await?;
        fs::rename(from, to).await?;
        Ok(())
    }

    pub async fn copy_file(from: &PathBuf, to: &PathBuf) -> anyhow::Result<()> {
        Self::create_parent_dirs(to).await?;
        fs::copy(from, to).await?;
        Ok(())
    }

    async fn create_parent_dirs(path: &PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(())
    }

    pub async fn get_files_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, String)>> {
        future::join_all(
            paths
                .into_iter()
                .map(async |file| Self::read_file(&file).await.map(|content| (file, content))),
        )
        .await
        .into_iter()
        .collect()
    }
}
