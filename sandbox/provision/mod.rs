pub mod artifact;
pub mod directory;
pub mod download;
pub mod image;
pub mod platform;

use anyhow::Context;
use std::path::PathBuf;

pub fn cache() -> anyhow::Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("Cannot locate Joe's sandbox cache")?
        .join("agent-joe/sandbox"))
}

#[cfg(test)]
mod tests;
