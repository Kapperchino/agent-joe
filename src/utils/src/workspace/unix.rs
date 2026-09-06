use super::*;
use rustix::fs::{self, AtFlags, FileType, Mode, OFlags};
use std::{
    ffi::OsString,
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
};

mod storage;
pub use storage::PrivateStorage;

struct Parent {
    directory: File,
    name: OsString,
}

enum Parents {
    Existing,
    Create,
}

struct OrdinaryFileMetadata {
    mode: Mode,
}

impl OrdinaryFileMetadata {
    fn new(stat: fs::Stat, path: &Path) -> anyhow::Result<Self> {
        if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile && stat.st_nlink == 1 {
            Ok(Self {
                mode: Mode::from_raw_mode(stat.st_mode & 0o777),
            })
        } else {
            Err(anyhow::anyhow!(
                "Only ordinary files with one link are supported: {}",
                path.display()
            ))
        }
    }
}

struct WorkspaceFile {
    handle: File,
    metadata: OrdinaryFileMetadata,
}

impl WorkspaceFile {
    fn open(path: ResolvedPath<'_>) -> anyhow::Result<Self> {
        let handle = path.open()?;
        let stat = fs::fstat(&handle).map_err(io_error)?;
        let metadata = OrdinaryFileMetadata::new(stat, &path.relative)?;
        Ok(Self { handle, metadata })
    }

    fn read_text(mut self) -> anyhow::Result<String> {
        let mut content = String::new();
        self.handle.read_to_string(&mut content)?;
        Ok(content)
    }

    fn read_bytes(mut self) -> anyhow::Result<Vec<u8>> {
        let mut content = Vec::new();
        self.handle.read_to_end(&mut content)?;
        Ok(content)
    }
}

impl Root {
    fn validate_identity(&self) -> anyhow::Result<()> {
        let pinned = fs::fstat(&self.directory).map_err(io_error)?;
        let current = fs::stat(&self.path).map_err(io_error)?;
        if pinned.st_dev == current.st_dev && pinned.st_ino == current.st_ino {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "A configured workspace root changed since initialization: {}",
                self.path.display()
            ))
        }
    }

    pub(super) fn open(spec: RootSpec, base: &Path) -> anyhow::Result<Self> {
        if spec.path.is_absolute() {
            let path = std::fs::canonicalize(&spec.path)?;
            if path.starts_with(base) {
                let directory =
                    fs::open(&path, directory_flags(), Mode::empty()).map_err(io_error)?;
                Ok(Self {
                    path,
                    alias: spec.path,
                    access: spec.access,
                    directory: directory.into(),
                })
            } else {
                Err(anyhow::anyhow!(
                    "Workspace roots must remain inside the project: {}",
                    path.display()
                ))
            }
        } else {
            Err(anyhow::anyhow!(
                "Workspace roots must be absolute: {}",
                spec.path.display()
            ))
        }
    }
}

fn io_error(error: rustix::io::Errno) -> anyhow::Error {
    std::io::Error::from(error).into()
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW
}

impl ResolvedPath<'_> {
    fn check_directory(&self, directory: File) -> anyhow::Result<File> {
        match self.access {
            Access::Read => Ok(directory),
            Access::Write => {
                let stat = fs::fstat(&directory).map_err(io_error)?;
                let protected = self
                    .policy
                    .roots
                    .iter()
                    .filter(|root| matches!(root.access, RootAccess::ReadOnly))
                    .try_fold(false, |protected, root| {
                        let root_stat = fs::fstat(&root.directory).map_err(io_error)?;
                        Ok::<_, anyhow::Error>(
                            protected
                                || (stat.st_dev == root_stat.st_dev
                                    && stat.st_ino == root_stat.st_ino),
                        )
                    })?;
                if protected {
                    Err(anyhow::anyhow!(
                        "Workspace write access denied for a read-only directory: {}",
                        self.relative.display()
                    ))
                } else {
                    Ok(directory)
                }
            }
        }
    }

    fn directory(&self, path: &Path, parents: Parents) -> anyhow::Result<File> {
        let root = self.check_directory(self.root.directory.try_clone()?)?;
        path.components()
            .try_fold(root, |directory, component| match component {
                Component::Normal(name) => {
                    match parents {
                        Parents::Existing => Ok(()),
                        Parents::Create => {
                            match fs::mkdirat(&directory, name, Mode::from_raw_mode(0o755)) {
                                Ok(()) | Err(rustix::io::Errno::EXIST) => Ok(()),
                                Err(error) => Err(io_error(error)),
                            }
                        }
                    }?;
                    let directory = fs::openat(&directory, name, directory_flags(), Mode::empty())
                        .map_err(io_error)?;
                    self.check_directory(directory.into())
                }
                Component::CurDir => Ok(directory),
                _ => Err(anyhow::anyhow!(
                    "Path traversal is not allowed: {}",
                    path.display()
                )),
            })
    }

    fn parent(&self, parents: Parents) -> anyhow::Result<Parent> {
        Parent::open(self, parents)
    }

    fn open(&self) -> anyhow::Result<File> {
        if self.relative.as_os_str().is_empty() {
            self.root.directory.try_clone().map_err(anyhow::Error::from)
        } else {
            let parent = self.parent(Parents::Existing)?;
            let file = fs::openat(
                &parent.directory,
                &parent.name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io_error)?;
            Ok(file.into())
        }
    }
}

impl Parent {
    fn open(path: &ResolvedPath<'_>, parents: Parents) -> anyhow::Result<Self> {
        let name = path.relative.file_name().ok_or_else(|| {
            anyhow::anyhow!(
                "An ordinary file path is required: {}",
                path.relative.display()
            )
        })?;
        let directory = path.directory(path.relative.parent().unwrap_or(Path::new("")), parents)?;
        Ok(Self {
            directory,
            name: name.into(),
        })
    }

    fn file_metadata(&self, path: &Path) -> anyhow::Result<OrdinaryFileMetadata> {
        let stat =
            fs::statat(&self.directory, &self.name, AtFlags::SYMLINK_NOFOLLOW).map_err(io_error)?;
        OrdinaryFileMetadata::new(stat, path)
    }

    fn writable_mode(&self, path: &Path) -> anyhow::Result<Mode> {
        match fs::statat(&self.directory, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => Ok(OrdinaryFileMetadata::new(stat, path)?.mode),
            Err(rustix::io::Errno::NOENT) => Ok(Mode::from_raw_mode(0o644)),
            Err(error) => Err(io_error(error)),
        }
    }

    fn write(&self, content: &[u8], mode: Mode) -> anyhow::Result<()> {
        let temporary = format!(".joe-write-{}", uuid::Uuid::new_v4());
        let result = self.write_temporary(&temporary, content, mode);
        if result.is_err() {
            let _ = fs::unlinkat(&self.directory, &temporary, AtFlags::empty());
        }
        result
    }

    fn write_temporary(&self, temporary: &str, content: &[u8], mode: Mode) -> anyhow::Result<()> {
        let fd = fs::openat(
            &self.directory,
            temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io_error)?;
        let mut file = File::from(fd);
        file.write_all(content)?;
        fs::fchmod(&file, mode).map_err(io_error)?;
        fs::renameat(&self.directory, temporary, &self.directory, &self.name).map_err(io_error)
    }
}

impl WorkspacePolicy {
    pub(crate) fn link_process_cache(
        &self,
        source: &Path,
        destination: &Path,
    ) -> anyhow::Result<()> {
        let parent = self
            .resolve(destination, Access::Write)?
            .parent(Parents::Create)?;
        match fs::symlinkat(source, &parent.directory, &parent.name) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => {
                let target = fs::readlinkat(&parent.directory, &parent.name, Vec::new())
                    .map_err(io_error)?;
                if target.to_bytes() == source.as_os_str().as_bytes() {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(
                        "The process cache location changed: {}",
                        destination.display()
                    ))
                }
            }
            Err(error) => Err(io_error(error)),
        }
    }

    pub(crate) fn validate_process_root(&self) -> anyhow::Result<()> {
        for root in &self.roots {
            root.validate_identity()?;
        }
        self.check(&self.base, Access::Read)?;
        let mut directories = vec![self.base.clone()];
        while let Some(directory) = directories.pop() {
            let directory_handle = self.resolve(&directory, Access::Read)?.open()?;
            for entry in self.entries(&directory)? {
                if self.check(&entry.path, Access::Read).is_ok() {
                    let stat =
                        fs::statat(&directory_handle, &entry.name, AtFlags::SYMLINK_NOFOLLOW)
                            .map_err(io_error)?;
                    match FileType::from_raw_mode(stat.st_mode) {
                        FileType::Directory => directories.push(entry.path),
                        FileType::RegularFile => {
                            OrdinaryFileMetadata::new(stat, &entry.path)?;
                        }
                        FileType::Symlink => {}
                        _ => Err(anyhow::anyhow!(
                            "Special files are not allowed in a process workspace: {}",
                            entry.path.display()
                        ))?,
                    }
                }
            }
        }
        Ok(())
    }

    pub fn open_append(&self, path: &Path) -> anyhow::Result<File> {
        let parent = self.resolve(path, Access::Write)?.parent(Parents::Create)?;
        let handle = fs::openat(
            &parent.directory,
            &parent.name,
            OFlags::WRONLY
                | OFlags::APPEND
                | OFlags::CREATE
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io_error)?;
        OrdinaryFileMetadata::new(fs::fstat(&handle).map_err(io_error)?, path)?;
        Ok(handle.into())
    }

    pub fn read(&self, path: &Path) -> anyhow::Result<String> {
        WorkspaceFile::open(self.resolve(path, Access::Read)?)?.read_text()
    }

    pub fn is_directory(&self, path: &Path) -> anyhow::Result<bool> {
        let file = self.resolve(path, Access::Read)?.open()?;
        Ok(file.metadata()?.is_dir())
    }

    pub fn entries(&self, path: &Path) -> anyhow::Result<Vec<DirectoryEntry>> {
        let resolved = self.resolve(path, Access::Read)?;
        let directory = resolved.directory(&resolved.relative, Parents::Existing)?;
        fs::Dir::read_from(directory)
            .map_err(io_error)?
            .filter_map(|entry| match entry {
                Ok(entry)
                    if entry.file_name().to_bytes() == b"."
                        || entry.file_name().to_bytes() == b".." =>
                {
                    None
                }
                entry => Some(
                    entry
                        .map(|entry| {
                            let name = std::ffi::OsStr::from_bytes(entry.file_name().to_bytes())
                                .to_os_string();
                            DirectoryEntry {
                                path: path.join(&name),
                                name,
                            }
                        })
                        .map_err(io_error),
                ),
            })
            .collect()
    }

    pub fn create_parent_dirs(&self, path: &Path) -> anyhow::Result<()> {
        self.resolve(path, Access::Write)?.parent(Parents::Create)?;
        Ok(())
    }

    pub fn write(&self, path: &Path, content: &str) -> anyhow::Result<()> {
        let parent = self.resolve(path, Access::Write)?.parent(Parents::Create)?;
        let mode = parent.writable_mode(path)?;
        parent.write(content.as_bytes(), mode)
    }

    pub fn copy(&self, source: &Path, destination: &Path) -> anyhow::Result<()> {
        let destination_path = self.resolve(destination, Access::Write)?;
        let source = WorkspaceFile::open(self.resolve(source, Access::Read)?)?;
        let mode = source.metadata.mode;
        let content = source.read_bytes()?;
        let parent = destination_path.parent(Parents::Create)?;
        parent.writable_mode(destination)?;
        parent.write(&content, mode)
    }

    pub fn delete(&self, path: &Path) -> anyhow::Result<()> {
        let parent = self
            .resolve(path, Access::Write)?
            .parent(Parents::Existing)?;
        parent.file_metadata(path)?;
        fs::unlinkat(&parent.directory, &parent.name, AtFlags::empty()).map_err(io_error)
    }

    pub fn rename(&self, source: &Path, destination: &Path) -> anyhow::Result<()> {
        let source_path = self.resolve(source, Access::Write)?;
        let destination_path = self.resolve(destination, Access::Write)?;
        let source_parent = source_path.parent(Parents::Existing)?;
        source_parent.file_metadata(source)?;
        let destination_parent = destination_path.parent(Parents::Create)?;
        destination_parent.writable_mode(destination)?;
        fs::renameat(
            &source_parent.directory,
            &source_parent.name,
            &destination_parent.directory,
            &destination_parent.name,
        )
        .map_err(io_error)
    }
}
