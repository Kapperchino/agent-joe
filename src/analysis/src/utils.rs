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
mod tests {
    use super::*;

    #[test]
    fn relative_path_uses_path_components() {
        let root = std::env::temp_dir().join("project");
        let path = root.join("src/lib.rs");
        let relative = RPath::new(path, root.to_string_lossy().into_owned()).unwrap();
        assert_eq!(
            relative.inner,
            PathBuf::from("src/lib.rs").to_str().unwrap()
        );
    }

    #[test]
    fn outside_path_error_identifies_the_file_and_root() {
        let directory = std::env::temp_dir();
        let root = directory.join("project");
        let path = directory.join("project-dependency/src/lib.rs");
        let error = RPath::new(path.clone(), root.to_string_lossy().into_owned()).unwrap_err();
        let message = error.to_string();
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains(&root.display().to_string()));
        assert!(message.contains("outside project root"));
    }
}

impl Display for RPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}
