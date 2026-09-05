use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
                .map(Root::open)
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(Self { base, roots })
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

    pub fn check(&self, path: &Path, access: Access) -> anyhow::Result<()> {
        self.resolve(path, access).map(|_| ())
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
            if protected(&absolute, access) {
                Err(anyhow::anyhow!(
                    "Protected workspace path: {}",
                    absolute.display()
                ))
            } else {
                policy
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
                    })
            }
        }
    }
}

#[cfg(unix)]
mod unix;

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

#[cfg(all(test, unix))]
mod tests;
