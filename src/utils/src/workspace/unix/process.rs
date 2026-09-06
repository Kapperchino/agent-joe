use super::*;
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

impl<'a> ProcessWorkspace<'a> {
    pub(crate) fn new(policy: &'a WorkspacePolicy) -> anyhow::Result<Self> {
        for root in &policy.roots {
            root.validate_identity()?;
        }
        policy.resolve(&policy.base, Access::Read)?;
        let mut links = HashMap::<LinkIdentity, FileLinks>::new();
        let mut directories = vec![policy.base.clone()];
        while let Some(directory) = directories.pop() {
            let directory_handle = policy.resolve(&directory, Access::Read)?.open()?;
            for entry in policy
                .entries(&directory)?
                .into_iter()
                .filter(|entry| policy.resolve(&entry.path, Access::Read).is_ok())
            {
                let stat = fs::statat(&directory_handle, &entry.name, AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(io_error)?;
                match FileType::from_raw_mode(stat.st_mode) {
                    FileType::Directory => directories.push(entry.path),
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
                        let file = links.entry(identity).or_insert_with(|| FileLinks {
                            path: entry.path,
                            expected: stat.st_nlink as u64,
                            observed: 0,
                        });
                        file.expected = file.expected.max(stat.st_nlink as u64);
                        file.observed += 1;
                    }
                    FileType::RegularFile | FileType::Symlink => {}
                    _ => Err(anyhow::anyhow!(
                        "Special files are not allowed in a process workspace: {}",
                        entry.path.display()
                    ))?,
                }
            }
        }
        match links.values().find(|file| file.observed != file.expected) {
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
