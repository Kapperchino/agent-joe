use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct RPath {
    pub inner: String,
}

impl RPath {
    pub fn new(path: PathBuf, root: String) -> anyhow::Result<RPath> {
        path.strip_prefix(&root)
            .with_context(|| format!("path {} is outside project root {root}", path.display()))?
            .to_str()
            .map(|relative| RPath {
                inner: relative.to_owned(),
            })
            .ok_or_else(|| anyhow!("invalid path: {}", path.display()))
    }
}

#[cfg(test)]
#[path = "../../../tests/analysis/utils/tests.rs"]
mod tests;

impl Display for RPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}
