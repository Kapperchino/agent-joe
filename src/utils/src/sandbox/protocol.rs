use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    pub library: PathBuf,
    pub rootfs: PathBuf,
    pub workspace: PathBuf,
    pub temporary_name: uuid::Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_identifiers_cannot_escape_the_temporary_directory() {
        for identifier in ["../outside", "/tmp/command", "", "command;exit"] {
            let configuration = serde_json::json!({
                "library": "/runtime/libkrun",
                "rootfs": "/runtime/rootfs",
                "workspace": "/workspace",
                "temporary_name": identifier,
            });
            assert!(serde_json::from_value::<Configuration>(configuration).is_err());
        }
    }
}
