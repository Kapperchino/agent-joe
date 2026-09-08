use super::*;
use crate::changes::{FileEdit, FileVersion};

pub struct StagedEdit {
    parent: Parent,
    replacement: Replacement,
}

enum Replacement {
    Delete,
    File { name: String },
}

impl Drop for StagedEdit {
    fn drop(&mut self) {
        if let Replacement::File { name } = &self.replacement {
            let _ = fs::unlinkat(&self.parent.directory, name, AtFlags::empty());
        }
    }
}

impl StagedEdit {
    pub fn apply(self, workspace: &WorkspacePolicy, edit: &FileEdit) -> anyhow::Result<()> {
        let current_parent = workspace
            .resolve(&edit.path, Access::Write)?
            .parent(Parents::Existing)?;
        let current = fs::fstat(&current_parent.directory).map_err(io_error)?;
        let staged = fs::fstat(&self.parent.directory).map_err(io_error)?;
        match current.st_dev == staged.st_dev
            && current.st_ino == staged.st_ino
            && workspace.file_version(&edit.path)? == edit.before
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Concurrent edit detected: {}",
                edit.path.display()
            )),
        }?;
        match &self.replacement {
            Replacement::Delete => {
                fs::unlinkat(&self.parent.directory, &self.parent.name, AtFlags::empty())
                    .map_err(io_error)?
            }
            Replacement::File { name } => fs::renameat(
                &self.parent.directory,
                name,
                &self.parent.directory,
                &self.parent.name,
            )
            .map_err(io_error)?,
        }
        self.parent.directory.sync_all()?;
        Ok(())
    }
}

impl WorkspacePolicy {
    pub fn file_version(&self, path: &Path) -> anyhow::Result<FileVersion> {
        let resolved = self.resolve(path, Access::Read)?;
        match WorkspaceFile::open(resolved) {
            Ok(file) => {
                let mode = file.metadata.mode.bits() as u32;
                Ok(FileVersion::File {
                    content: file.read_bytes()?,
                    mode,
                })
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(FileVersion::Missing)
            }
            Err(error) => Err(error),
        }
    }

    pub fn stage_edit(&self, edit: &FileEdit) -> anyhow::Result<StagedEdit> {
        let parent = self
            .resolve(&edit.path, Access::Write)?
            .parent(Parents::Create)?;
        parent.writable_mode(&edit.path)?;
        let mut staged = StagedEdit {
            parent,
            replacement: Replacement::Delete,
        };
        if let FileVersion::File { content, mode } = &edit.after {
            let name = format!(".joe-write-{}", uuid::Uuid::new_v4());
            staged.replacement = Replacement::File { name: name.clone() };
            let fd = fs::openat(
                &staged.parent.directory,
                &name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::from_raw_mode(0o600),
            )
            .map_err(io_error)?;
            let mut file = File::from(fd);
            file.write_all(content)?;
            fs::fchmod(&file, Mode::from_raw_mode((*mode & 0o777) as _)).map_err(io_error)?;
            file.sync_all()?;
        }
        Ok(staged)
    }
}
