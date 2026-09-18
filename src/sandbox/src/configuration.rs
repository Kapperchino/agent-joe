use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) const LAUNCHER_PROTOCOL_VERSION: &str = "1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    pub firmware: PathBuf,
    pub init: PathBuf,
    pub rootfs: PathBuf,
    pub workspace: PathBuf,
    pub cache: PathBuf,
}
