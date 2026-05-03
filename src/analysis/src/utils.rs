use anyhow::anyhow;
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
        let rpath = path
            .strip_prefix(&(root.to_owned() + "/"))?
            .to_path_buf()
            .to_str()
            .ok_or_else(|| anyhow!("invalid path"))?
            .to_string();
        Ok(RPath { inner: rpath })
    }
}

impl Display for RPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}
