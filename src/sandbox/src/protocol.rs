use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    pub firmware: PathBuf,
    pub init: PathBuf,
    pub rootfs: PathBuf,
    pub workspace: PathBuf,
    pub temporary_name: uuid::Uuid,
}

#[cfg(test)]
#[path = "../tests/unit/protocol/tests.rs"]
mod tests;
