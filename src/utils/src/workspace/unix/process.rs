use super::*;
use anyhow::Context;
use std::collections::HashMap;

pub(crate) struct ProcessWorkspace<'a> {
    policy: &'a WorkspacePolicy,
}

#[derive(PartialEq, Eq, Hash)]
struct LinkIdentity {
    device: u64,
    inode: u64,
    access: Access,
}

struct FileLinks {
    path: PathBuf,
    expected: u64,
    observed: u64,
}

struct WorkspaceScan {
    links: HashMap<LinkIdentity, FileLinks>,
    directories: Vec<PathBuf>,
}

impl<'a> ProcessWorkspace<'a> {
    pub(crate) fn new(policy: &'a WorkspacePolicy) -> anyhow::Result<Self> {
        for root in &policy.roots {
            root.validate_identity()?;
        }
        policy.resolve(&policy.base, Access::Read)?;
        let mut scan = WorkspaceScan {
            links: HashMap::new(),
            directories: vec![policy.base.clone()],
        };
        while let Some(directory) = scan.directories.pop() {
            match scan.read(policy, &directory) {
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                result => result?,
            }
        }
        match scan
            .links
            .values()
            .find(|file| file.observed != file.expected)
        {
            Some(file) => Err(anyhow::anyhow!(
                "Hard links must remain within workspace paths with the same access: {} ({} of {} links found)",
                file.path.display(),
                file.observed,
                file.expected,
            )),
            None => Ok(Self { policy }),
        }
    }

    pub(crate) fn policy(&self) -> &'a WorkspacePolicy {
        self.policy
    }
}

impl WorkspaceScan {
    fn read(&mut self, policy: &WorkspacePolicy, directory: &Path) -> anyhow::Result<()> {
        let directory_handle = policy
            .resolve(directory, Access::Read)?
            .open()
            .with_context(|| {
                format!(
                    "Cannot open process workspace directory {}",
                    directory.display()
                )
            })?;
        for entry in policy
            .entries(directory)
            .with_context(|| {
                format!(
                    "Cannot read process workspace directory {}",
                    directory.display()
                )
            })?
            .into_iter()
            .filter(|entry| policy.resolve(&entry.path, Access::Read).is_ok())
        {
            match fs::statat(&directory_handle, &entry.name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => self.record(policy, entry, stat)?,
                Err(rustix::io::Errno::NOENT) => {}
                Err(error) => Err(io_error(error)).with_context(|| {
                    format!(
                        "Cannot inspect process workspace entry {}",
                        entry.path.display()
                    )
                })?,
            }
        }
        Ok(())
    }

    fn record(
        &mut self,
        policy: &WorkspacePolicy,
        entry: DirectoryEntry,
        stat: fs::Stat,
    ) -> anyhow::Result<()> {
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {
                self.directories.push(entry.path);
                Ok(())
            }
            FileType::RegularFile if stat.st_nlink > 1 => {
                let access = match policy.resolve(&entry.path, Access::Write) {
                    Ok(_) => Access::Write,
                    Err(_) => Access::Read,
                };
                let identity = LinkIdentity {
                    device: stat.st_dev as u64,
                    inode: stat.st_ino as u64,
                    access,
                };
                let file = self.links.entry(identity).or_insert_with(|| FileLinks {
                    path: entry.path,
                    expected: stat.st_nlink as u64,
                    observed: 0,
                });
                file.expected = file.expected.max(stat.st_nlink as u64);
                file.observed += 1;
                Ok(())
            }
            FileType::RegularFile | FileType::Symlink => Ok(()),
            _ => Err(anyhow::anyhow!(
                "Special files are not allowed in a process workspace: {}",
                entry.path.display()
            )),
        }
    }
}
