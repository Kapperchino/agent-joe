use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy)]
pub enum RootAccess {
    ReadOnly,
    ReadWrite,
}

pub struct RootSpec {
    pub path: PathBuf,
    pub access: RootAccess,
}

pub struct WorkspacePolicy {
    base: PathBuf,
    roots: Vec<Root>,
    restrictions: Vec<PathRestriction>,
}

#[derive(Clone)]
struct PathRestriction {
    paths: Vec<PathBuf>,
    access: RootAccess,
}

struct Root {
    path: PathBuf,
    alias: PathBuf,
    access: RootAccess,
    directory: File,
}

struct ResolvedPath<'a> {
    policy: &'a WorkspacePolicy,
    root: &'a Root,
    relative: PathBuf,
    access: Access,
}

impl WorkspacePolicy {
    pub fn workspace(path: PathBuf) -> anyhow::Result<Self> {
        Self::new(
            path.clone(),
            vec![RootSpec {
                path,
                access: RootAccess::ReadWrite,
            }],
        )
    }

    pub fn new(base: PathBuf, roots: Vec<RootSpec>) -> anyhow::Result<Self> {
        #[cfg(unix)]
        {
            let base = std::fs::canonicalize(&base)?;
            let roots = roots
                .into_iter()
                .map(|spec| Root::open(spec, &base))
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(Self {
                base,
                roots,
                restrictions: Vec::new(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = base;
            let _ = roots;
            Err(anyhow::anyhow!(
                "Descriptor-based workspace access is unsupported on this platform"
            ))
        }
    }

    pub fn root(&self) -> &Path {
        &self.base
    }

    pub fn restricted(&self, paths: &[PathBuf], access: RootAccess) -> anyhow::Result<Self> {
        let paths = paths
            .iter()
            .map(|path| {
                self.relative_path(path, Access::Read)
                    .map(|path| self.base.join(path))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        match paths.is_empty() {
            true => Err(anyhow::anyhow!("Worker paths cannot be empty")),
            false => {
                let roots = self
                    .roots
                    .iter()
                    .map(|root| {
                        Ok(Root {
                            path: root.path.clone(),
                            alias: root.alias.clone(),
                            access: root.access,
                            directory: root.directory.try_clone()?,
                        })
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let mut restrictions = self.restrictions.clone();
                restrictions.push(PathRestriction { paths, access });
                Ok(Self {
                    base: self.base.clone(),
                    roots,
                    restrictions,
                })
            }
        }
    }

    pub fn permits_workspace_execution(&self) -> bool {
        self.permits_workspace_access(Access::Write)
    }

    pub fn permits_workspace_access(&self, access: Access) -> bool {
        self.resolve(&self.base, access).is_ok()
            && self.restrictions.iter().all(|restriction| {
                (access == Access::Read || matches!(restriction.access, RootAccess::ReadWrite))
                    && restriction.paths.iter().any(|path| path == &self.base)
            })
    }

    pub(crate) fn read_only_roots(&self) -> impl Iterator<Item = &Path> {
        self.roots
            .iter()
            .filter(|root| matches!(root.access, RootAccess::ReadOnly))
            .map(|root| root.path.as_path())
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn process_protected_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut directories = vec![self.base.clone()];
        let mut paths = Vec::new();
        while let Some(directory) = directories.pop() {
            for entry in self.entries(&directory)? {
                if protected(&entry.path, Access::Write) {
                    let metadata = std::fs::symlink_metadata(&entry.path)?;
                    if metadata.is_symlink() {
                        Err(anyhow::anyhow!(
                            "Protected process paths cannot be symlinks: {}",
                            entry.path.display()
                        ))?;
                    }
                    paths.push(entry.path);
                } else if self.is_directory(&entry.path).unwrap_or(false) {
                    directories.push(entry.path);
                }
            }
        }
        Ok(paths)
    }

    pub fn check(&self, path: &Path, access: Access) -> anyhow::Result<()> {
        self.resolve(path, access).map(|_| ())
    }

    pub fn relative_path(&self, path: &Path, access: Access) -> anyhow::Result<PathBuf> {
        let resolved = self.resolve(path, access)?;
        Ok(resolved
            .root
            .path
            .join(resolved.relative)
            .strip_prefix(&self.base)?
            .to_path_buf())
    }

    fn resolve(&self, path: &Path, access: Access) -> anyhow::Result<ResolvedPath<'_>> {
        ResolvedPath::new(self, path, access)
    }
}

impl<'a> ResolvedPath<'a> {
    fn new(policy: &'a WorkspacePolicy, path: &Path, access: Access) -> anyhow::Result<Self> {
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            Err(anyhow::anyhow!(
                "Path traversal is not allowed: {}",
                path.display()
            ))
        } else {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                policy.base.join(path)
            };
            let absolute: PathBuf = absolute.components().collect();
            let allowed = policy.restrictions.iter().all(|restriction| {
                (access == Access::Read || matches!(restriction.access, RootAccess::ReadWrite))
                    && restriction.paths.iter().any(|path| {
                        absolute.starts_with(path)
                            || (access == Access::Read && path.starts_with(&absolute))
                    })
            });
            match allowed {
                false => Err(anyhow::anyhow!(
                    "Worker path access denied for {access:?}: {}",
                    absolute.display()
                )),
                true if protected(&absolute, access) => Err(anyhow::anyhow!(
                    "Protected workspace path: {}",
                    absolute.display()
                )),
                true => policy
                    .roots
                    .iter()
                    .filter_map(|root| {
                        absolute
                            .strip_prefix(&root.path)
                            .or_else(|_| absolute.strip_prefix(&root.alias))
                            .ok()
                            .map(|relative| Self {
                                policy,
                                root,
                                relative: relative.into(),
                                access,
                            })
                    })
                    .min_by_key(|resolved| resolved.relative.components().count())
                    .filter(|resolved| {
                        (access == Access::Read
                            || matches!(resolved.root.access, RootAccess::ReadWrite))
                            && !protected(&resolved.root.path.join(&resolved.relative), access)
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Workspace access denied for {access:?}: {}",
                            absolute.display()
                        )
                    }),
            }
        }
    }
}

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::PrivateStorage;

#[cfg(unix)]
pub(crate) use unix::ProcessWorkspace;

pub struct DirectoryEntry {
    pub name: std::ffi::OsString,
    pub path: PathBuf,
}

fn protected(path: &Path, access: Access) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name.eq_ignore_ascii_case(crate::utils::CONFIG_DIR_NAME)
                || (access == Access::Write
                    && [".git", ".agents", ".codex"]
                        .iter()
                        .any(|protected| name.eq_ignore_ascii_case(protected)))
        }
        _ => false,
    })
}

#[cfg(not(unix))]
mod unsupported;

#[cfg(not(unix))]
pub use unsupported::PrivateStorage;

#[cfg(all(test, unix))]
mod tests;
