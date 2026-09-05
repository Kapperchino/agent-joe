use futures::future;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::wrappers::ReadDirStream;

pub struct Files {}

impl Files {
    pub async fn delete_file(file: &PathBuf) -> anyhow::Result<()> {
        let file = file.clone();
        mutation(move || std::fs::remove_file(file)).await
    }

    pub async fn create_file(file: &PathBuf, data: &str) -> anyhow::Result<()> {
        let file = file.clone();
        mutation(move || std::fs::File::create(file).map(|_| ())).await
    }

    pub async fn read_file(file: &PathBuf) -> anyhow::Result<String> {
        fs::read_to_string(file).await.map_err(Into::into)
    }

    pub async fn get_dir_files(dir: &PathBuf) -> anyhow::Result<Vec<DirEntry>> {
        use tokio_stream::StreamExt;

        match fs::read_dir(dir).await {
            Ok(read_dir) => Ok(ReadDirStream::new(read_dir)
                .fold(vec![], |mut entries, item| {
                    match item {
                        Ok(entry) => entries.push(entry),
                        Err(_) => println!("error with getting files"),
                    }
                    entries
                })
                .await),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn write_to_file(dir: &PathBuf, content: &str) -> anyhow::Result<()> {
        let path = dir.clone();
        let content = content.to_owned();
        mutation(move || std::fs::write(path, content)).await
    }

    pub async fn rename_file(from: &PathBuf, to: &PathBuf) -> anyhow::Result<()> {
        match Self::create_parent_dirs(to).await {
            Ok(()) => {
                let (from, to) = (from.clone(), to.clone());
                mutation(move || std::fs::rename(from, to)).await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn copy_file(from: &PathBuf, to: &PathBuf) -> anyhow::Result<()> {
        match Self::create_parent_dirs(to).await {
            Ok(()) => {
                let (from, to) = (from.clone(), to.clone());
                mutation(move || std::fs::copy(from, to).map(|_| ())).await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn create_parent_dirs(path: &PathBuf) -> anyhow::Result<()> {
        match path.parent() {
            Some(parent) => {
                let parent = parent.to_path_buf();
                mutation(move || std::fs::create_dir_all(parent)).await
            }
            None => Ok(()),
        }
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

async fn mutation<F>(operation: F) -> anyhow::Result<()>
where
    F: FnOnce() -> std::io::Result<()> + Send + 'static,
{
    let scope = crate::execution::ExecutionScope::current();
    if scope.cancel.is_cancelled() {
        Err(anyhow::anyhow!(
            "Filesystem operation cancelled before execution"
        ))
    } else {
        scope
            .tasks
            .spawn_blocking(operation)
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result.map_err(Into::into))
    }
}
