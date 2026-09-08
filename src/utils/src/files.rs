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
        let changes = ExecutionScope::current().changes;
        operation(move |workspace| {
            let before = workspace.file_version(&path)?;
            let edit = crate::changes::FileEdit::new(
                workspace,
                &path,
                before,
                crate::changes::FileVersion::Missing,
            )?;
            changes.apply(workspace, vec![edit]).map(|_| ())
        })
        .await
    }

    pub async fn create_file(file: &Path, data: &str) -> anyhow::Result<()> {
        Self::write_to_file(file, data).await
    }

    pub async fn read_file(file: &Path) -> anyhow::Result<String> {
        let path = file.to_path_buf();
        let changes = ExecutionScope::current().changes;
        operation(move |workspace| {
            let version = workspace.file_version(&path)?;
            let text = version.text()?.to_owned();
            changes.observe(workspace, &path, version)?;
            Ok(text)
        })
        .await
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
        let changes = ExecutionScope::current().changes;
        operation(move |workspace| {
            let before = workspace.file_version(&path)?;
            let after = before.with_text(content);
            let edit = crate::changes::FileEdit::new(workspace, &path, before, after)?;
            changes.apply(workspace, vec![edit]).map(|_| ())
        })
        .await
    }

    pub async fn rename_file(from: &Path, to: &Path) -> anyhow::Result<()> {
        let source = from.to_path_buf();
        let destination = to.to_path_buf();
        let changes = ExecutionScope::current().changes;
        operation(move |workspace| {
            let before = workspace.file_version(&source)?;
            match before != crate::changes::FileVersion::Missing {
                true => Ok(()),
                false => Err(anyhow::anyhow!("Move source does not exist")),
            }?;
            let destination_before = workspace.file_version(&destination)?;
            match destination_before == crate::changes::FileVersion::Missing {
                true => Ok(()),
                false => Err(anyhow::anyhow!("Move destination already exists")),
            }?;
            let add = crate::changes::FileEdit::new(
                workspace,
                &destination,
                destination_before,
                before.clone(),
            )?;
            let delete = crate::changes::FileEdit::new(
                workspace,
                &source,
                before,
                crate::changes::FileVersion::Missing,
            )?;
            changes.apply(workspace, vec![add, delete]).map(|_| ())
        })
        .await
    }

    pub async fn copy_file(from: &Path, to: &Path) -> anyhow::Result<()> {
        let source = from.to_path_buf();
        let destination = to.to_path_buf();
        let changes = ExecutionScope::current().changes;
        operation(move |workspace| {
            let after = workspace.file_version(&source)?;
            match after != crate::changes::FileVersion::Missing {
                true => Ok(()),
                false => Err(anyhow::anyhow!("Copy source does not exist")),
            }?;
            let before = workspace.file_version(&destination)?;
            let edit = crate::changes::FileEdit::new(workspace, &destination, before, after)?;
            changes.apply(workspace, vec![edit]).map(|_| ())
        })
        .await
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

pub async fn operation<T, F>(operation: F) -> anyhow::Result<T>
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
