use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checksum {
    hex: String,
}

impl Checksum {
    pub fn new(hex: &str) -> anyhow::Result<Self> {
        match hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            true => Ok(Self {
                hex: hex.to_ascii_lowercase(),
            }),
            false => Err(anyhow::anyhow!("Invalid sandbox artifact checksum")),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.hex
    }
}

pub struct Artifact {
    url: reqwest::Url,
    checksum: Checksum,
}

impl Artifact {
    pub fn new(url: &str, checksum: &str) -> anyhow::Result<Self> {
        let url = reqwest::Url::parse(url)?;
        let checksum = Checksum::new(checksum)?;
        match url.scheme() {
            "https" => Ok(Self { url, checksum }),
            _ => Err(anyhow::anyhow!("Sandbox artifacts require HTTPS")),
        }
    }

    pub fn url(&self) -> &reqwest::Url {
        &self.url
    }

    pub fn checksum(&self) -> &Checksum {
        &self.checksum
    }
}

pub struct ArchivePath {
    path: PathBuf,
}

impl ArchivePath {
    pub fn new(path: PathBuf) -> anyhow::Result<Self> {
        match path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
        {
            true => Ok(Self { path }),
            false => Err(anyhow::anyhow!(
                "Unsafe sandbox archive entry: {}",
                path.display()
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
