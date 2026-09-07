use crate::workspace::WorkspacePolicy;
use std::path::{Path, PathBuf};

pub(super) struct CargoCache {
    pub(super) target: PathBuf,
    pub(super) home: PathBuf,
}

impl CargoCache {
    pub(super) fn new(workspace: &WorkspacePolicy, cargo_home: &Path) -> anyhow::Result<Self> {
        let cache = Self {
            target: workspace.root().join("target/.joe/build"),
            home: workspace.root().join("target/.joe/cargo"),
        };
        for path in [&cache.target, &cache.home] {
            workspace.create_parent_dirs(&path.join("placeholder"))?;
        }
        for directory in ["index", "cache"] {
            let source = cargo_home.join("registry").join(directory);
            if source.exists() {
                workspace.link_process_cache(
                    &source.canonicalize()?,
                    &cache.home.join("registry").join(directory),
                )?;
            }
        }
        Ok(cache)
    }
}
