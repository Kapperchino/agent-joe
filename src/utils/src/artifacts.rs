use serde::{Deserialize, Serialize};

pub const ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactReference {
    pub id: String,
    pub bytes: usize,
}

impl ArtifactReference {
    pub fn new(id: String, bytes: usize) -> anyhow::Result<Self> {
        match bytes <= ARTIFACT_BYTES {
            true => Ok(Self { id, bytes }),
            false => Err(anyhow::anyhow!("Output exceeds the 64 MiB artifact limit")),
        }
    }
}
