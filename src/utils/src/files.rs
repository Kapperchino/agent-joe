use crate::{
    execution::ExecutionScope,
    workspace::{DirectoryEntry, WorkspacePolicy},
};
use std::path::{Path, PathBuf};

pub struct Files;

pub struct FileContent {
    pub path: PathBuf,
    pub content: String,
}

impl Files {
    pub async fn delete_file(file: &Path) -> anyhow::Result<()> {
        let path = file.to_path_buf();
        operation(move |workspace| workspace.delete(&path)).await
    }

    pub async fn create_file(file: &Path, data: &str) -> anyhow::Result<()> {
        Self::write_to_file(file, data).await
    }

    pub async fn read_file(file: &Path) -> anyhow::Result<String> {
        let path = file.to_path_buf();
        operation(move |workspace| workspace.read(&path)).await
    }

    pub fn read_file_sync(file: &Path) -> anyhow::Result<String> {
        let scope = ExecutionScope::current();
        if scope.cancel.is_cancelled() {
            Err(anyhow::anyhow!(
                "Filesystem operation cancelled before execution"
            ))
        } else {
            scope.workspace()?.read(file)
        }
    }

    pub async fn is_directory(path: &Path) -> anyhow::Result<bool> {
        let path = path.to_path_buf();
        operation(move |workspace| workspace.is_directory(&path)).await
    }

    pub async fn get_dir_files(dir: &Path) -> anyhow::Result<Vec<DirectoryEntry>> {
        let path = dir.to_path_buf();
        operation(move |workspace| workspace.entries(&path)).await
    }

    pub async fn write_to_file(path: &Path, content: &str) -> anyhow::Result<()> {
        let path = path.to_path_buf();
        let content = content.to_owned();
        operation(move |workspace| workspace.write(&path, &content)).await
    }

    pub async fn rename_file(from: &Path, to: &Path) -> anyhow::Result<()> {
        let source = from.to_path_buf();
        let destination = to.to_path_buf();
        operation(move |workspace| workspace.rename(&source, &destination)).await
    }

    pub async fn copy_file(from: &Path, to: &Path) -> anyhow::Result<()> {
        let source = from.to_path_buf();
        let destination = to.to_path_buf();
        operation(move |workspace| workspace.copy(&source, &destination)).await
    }

    pub async fn create_parent_dirs(path: &Path) -> anyhow::Result<()> {
        let path = path.to_path_buf();
        operation(move |workspace| workspace.create_parent_dirs(&path)).await
    }

    pub async fn get_files_for_paths(paths: Vec<PathBuf>) -> anyhow::Result<Vec<FileContent>> {
        futures::future::join_all(paths.into_iter().map(async |path| {
            let content = Self::read_file(&path).await?;
            Ok(FileContent { path, content })
        }))
        .await
        .into_iter()
        .collect()
    }
}

async fn operation<T, F>(operation: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce(&WorkspacePolicy) -> anyhow::Result<T> + Send + 'static,
{
    let scope = ExecutionScope::current();
    if scope.cancel.is_cancelled() {
        Err(anyhow::anyhow!(
            "Filesystem operation cancelled before execution"
        ))
    } else {
        let workspace = scope.workspace()?;
        scope
            .tasks
            .spawn_blocking(move || operation(&workspace))
            .await?
    }
}
