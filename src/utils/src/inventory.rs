use crate::workspace::{Access, WorkspacePolicy};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 250_000;
const MAX_PATH_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub files: Vec<PathBuf>,
    pub skipped: usize,
}

struct Directory {
    path: PathBuf,
    rules: Vec<Gitignore>,
}

enum InventoryMode {
    Discovery,
    Git,
}

impl Inventory {
    pub fn scan(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        Self::scan_with(workspace, InventoryMode::Discovery)
    }

    pub fn scan_git(workspace: &WorkspacePolicy) -> anyhow::Result<Self> {
        Self::scan_with(workspace, InventoryMode::Git)
    }

    fn scan_with(workspace: &WorkspacePolicy, mode: InventoryMode) -> anyhow::Result<Self> {
        let mut pending = vec![Directory {
            path: workspace.root().to_path_buf(),
            rules: Vec::new(),
        }];
        let mut inventory = Self {
            files: Vec::new(),
            skipped: 0,
        };
        let mut visited = 0;
        let mut path_bytes = 0;
        while let Some(mut directory) = pending.pop() {
            let mut entries = workspace.entries(&directory.path)?;
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            visited += entries.len();
            match visited <= MAX_ENTRIES {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Inventory exceeds {MAX_ENTRIES} entries; add ignore rules"
                )),
            }?;
            let ignore_files: &[&str] = match mode {
                InventoryMode::Discovery => &[".gitignore", ".ignore"],
                InventoryMode::Git => &[".gitignore"],
            };
            if matches!(mode, InventoryMode::Git)
                && entries.iter().any(|entry| entry.name == ".gitattributes")
            {
                workspace.read(&directory.path.join(".gitattributes"))?;
            }
            for name in ignore_files {
                if entries.iter().any(|entry| entry.name == *name)
                    && workspace
                        .check(&directory.path.join(name), Access::Read)
                        .is_ok()
                {
                    let path = directory.path.join(name);
                    let mut builder = GitignoreBuilder::new(&directory.path);
                    for line in workspace.read(&path)?.lines() {
                        builder.add_line(Some(path.clone()), line)?;
                    }
                    directory.rules.push(builder.build()?);
                }
            }
            for entry in entries {
                let kind = workspace.is_directory(&entry.path);
                let excluded = [".git", ".turbo-code", ".joe-worktrees"]
                    .iter()
                    .any(|name| entry.name.eq_ignore_ascii_case(name))
                    || (matches!(mode, InventoryMode::Discovery)
                        && entry.name.eq_ignore_ascii_case("target"));
                match kind {
                    Ok(is_directory)
                        if !excluded && workspace.check(&entry.path, Access::Read).is_ok() =>
                    {
                        let ignored = directory
                            .rules
                            .iter()
                            .rev()
                            .map(|rules| rules.matched(&entry.path, is_directory))
                            .find(|matched| !matched.is_none())
                            .is_some_and(|matched| matched.is_ignore());
                        match (ignored, is_directory) {
                            (true, _) => {}
                            (false, true) => pending.push(Directory {
                                path: entry.path,
                                rules: directory.rules.clone(),
                            }),
                            (false, false) if workspace.file_size(&entry.path).is_err() => {
                                inventory.skipped += 1
                            }
                            (false, false) => {
                                let path = entry.path.strip_prefix(workspace.root())?.to_path_buf();
                                path_bytes += path.as_os_str().len();
                                match path_bytes <= MAX_PATH_BYTES {
                                    true => Ok(()),
                                    false => Err(anyhow::anyhow!(
                                        "Inventory paths exceed 32 MiB; add ignore rules"
                                    )),
                                }?;
                                inventory.files.push(path);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(_) => inventory.skipped += 1,
                }
            }
        }
        inventory.files.sort();
        Ok(inventory)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Listing {
    pub entries: Vec<ListedEntry>,
    pub total: usize,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListedEntry {
    pub path: PathBuf,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Copy)]
pub struct ResultLimit(usize);

impl ResultLimit {
    pub fn new(limit: Option<usize>) -> anyhow::Result<Self> {
        let limit = limit.unwrap_or(200);
        match (1..=1000).contains(&limit) {
            true => Ok(Self(limit)),
            false => Err(anyhow::anyhow!("Result limit must be between 1 and 1000")),
        }
    }

    pub fn get(self) -> usize {
        self.0
    }
}

impl Listing {
    pub fn read(
        workspace: &WorkspacePolicy,
        path: &Path,
        offset: usize,
        limit: ResultLimit,
    ) -> anyhow::Result<Self> {
        let mut entries = workspace
            .entries(path)?
            .into_iter()
            .filter_map(|entry| {
                workspace
                    .is_directory(&entry.path)
                    .ok()
                    .filter(|directory| *directory || workspace.file_size(&entry.path).is_ok())
                    .map(|directory| ListedEntry {
                        path: workspace
                            .relative_path(&entry.path, Access::Read)
                            .unwrap_or(entry.path),
                        kind: if directory {
                            EntryKind::Directory
                        } else {
                            EntryKind::File
                        },
                    })
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let total = entries.len();
        let entries = entries
            .into_iter()
            .skip(offset)
            .take(limit.get())
            .collect::<Vec<_>>();
        let end = offset.saturating_add(entries.len());
        Ok(Self {
            entries,
            total,
            truncated: end < total,
            next_offset: (end < total).then_some(end),
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/inventory/tests.rs"]
mod tests;
