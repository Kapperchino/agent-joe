use super::provision::{
    artifact::{Artifact, Checksum},
    download::Downloads,
};
use crate::{ProcessLimits, Sandbox, workspace::Workspace};
use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const INDEX: &str = "index.crates.io-1949cf8c6b5b557f";

#[derive(Default, Deserialize)]
struct Lockfile {
    package: Vec<LockedPackage>,
}

impl Lockfile {
    fn read(workspace: &dyn Workspace) -> anyhow::Result<Self> {
        match workspace.root().join("Cargo.lock").exists() {
            true => Ok(toml::from_str(&workspace.read(Path::new("Cargo.lock"))?)?),
            false => Ok(Self::default()),
        }
    }
}

#[derive(Deserialize)]
struct LockedPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RegistryIndex {
    name: String,
}

impl RegistryIndex {
    fn new(name: String) -> anyhow::Result<Self> {
        match !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            true => Ok(Self { name }),
            false => Err(anyhow::anyhow!("Invalid registry package name")),
        }
    }

    fn from_diagnostic(stderr: &str) -> anyhow::Result<Option<Self>> {
        let missing = stderr
            .split("no matching package named `")
            .nth(1)
            .and_then(|message| message.split('`').next());
        let outdated = stderr
            .split("failed to select a version for the requirement `")
            .nth(1)
            .and_then(|message| message.split([' ', '=', '`']).next());
        missing
            .or(outdated)
            .map(|name| Self::new(name.into()))
            .transpose()
    }

    fn path(&self) -> String {
        let name = self.name.to_ascii_lowercase();
        match name.len() {
            1 => format!("1/{name}"),
            2 => format!("2/{name}"),
            3 => format!("3/{}/{name}", &name[..1]),
            _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
        }
    }
}

struct RegistryPackage {
    index: RegistryIndex,
    version: String,
    checksum: Checksum,
}

impl RegistryPackage {
    fn new(package: LockedPackage) -> anyhow::Result<Self> {
        let index = RegistryIndex::new(package.name)?;
        let checksum = Checksum::new(
            &package
                .checksum
                .context("Registry dependency has no locked checksum")?,
        )?;
        match !package.version.is_empty()
            && package.version.len() <= 128
            && package
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
        {
            true => Ok(Self {
                index,
                version: package.version,
                checksum,
            }),
            false => Err(anyhow::anyhow!("Invalid locked registry dependency")),
        }
    }

    fn matches(&self, bytes: &[u8]) -> bool {
        serde_json::from_slice::<IndexEntry>(bytes).is_ok_and(|entry| {
            entry.name == self.index.name
                && entry.vers == self.version
                && entry.cksum == self.checksum.as_str()
        })
    }

    fn install(&self, registry: &RegistryCache, downloads: &Downloads<'_>) -> anyhow::Result<()> {
        downloads.check()?;
        let index = registry.index_path(&self.index);
        let contents = match index.is_file() {
            true => fs::read(&index)?,
            false => Vec::new(),
        };
        if !contents
            .split(|byte| *byte == 0)
            .any(|entry| self.matches(entry))
        {
            registry.install_index(&self.index, Some(self), downloads)?;
        }
        let filename = format!("{}-{}.crate", self.index.name, self.version);
        let archive = registry.root.join("cache").join(INDEX).join(&filename);
        if !archive.is_file() {
            let artifact = Artifact::new(
                &format!(
                    "https://static.crates.io/crates/{}/{filename}",
                    self.index.name
                ),
                self.checksum.as_str(),
            )?;
            let source = downloads.get(&artifact, None)?;
            let temporary = archive.with_extension("partial");
            fs::copy(source, &temporary)?;
            fs::rename(temporary, archive)?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct IndexEntry {
    name: String,
    vers: String,
    cksum: String,
}

struct SparseIndex {
    bytes: Vec<u8>,
}

impl SparseIndex {
    fn new(response: &str, package: Option<&RegistryPackage>) -> anyhow::Result<Self> {
        match package.is_none_or(|package| {
            response
                .lines()
                .any(|line| package.matches(line.as_bytes()))
        }) {
            true => {
                let bytes = response.lines().try_fold(
                    b"\x03\x02\0\0\0etag: joe\0".to_vec(),
                    |mut bytes, line| {
                        let entry: IndexEntry = serde_json::from_str(line)?;
                        bytes.extend(entry.vers.as_bytes());
                        bytes.push(0);
                        bytes.extend(line.as_bytes());
                        bytes.push(0);
                        Ok::<_, anyhow::Error>(bytes)
                    },
                )?;
                Ok(Self { bytes })
            }
            false => Err(anyhow::anyhow!(
                "Registry metadata does not match the locked package checksum"
            )),
        }
    }
}

struct RegistryCache {
    root: PathBuf,
}

impl RegistryCache {
    fn new(rootfs: &Path) -> anyhow::Result<Self> {
        let root = rootfs.join("usr/local/cargo/registry");
        for directory in ["cache", "index"] {
            fs::create_dir_all(root.join(directory).join(INDEX))?;
        }
        atomic_write(
            &root.join("index").join(INDEX).join("config.json"),
            br#"{"dl":"https://static.crates.io/crates","api":"https://crates.io"}"#,
        )?;
        Ok(Self { root })
    }

    fn index_path(&self, index: &RegistryIndex) -> PathBuf {
        self.root
            .join("index")
            .join(INDEX)
            .join(".cache")
            .join(index.path())
    }

    fn install_index(
        &self,
        index: &RegistryIndex,
        package: Option<&RegistryPackage>,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<()> {
        let response = downloads.text(&format!("https://index.crates.io/{}", index.path()))?;
        let contents = SparseIndex::new(&response, package)?;
        atomic_write(&self.index_path(index), &contents.bytes)
    }

    fn install_packages(
        &self,
        workspace: &dyn Workspace,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<()> {
        Lockfile::read(workspace)?
            .package
            .into_iter()
            .filter(|package| {
                package.source.as_deref()
                    == Some("registry+https://github.com/rust-lang/crates.io-index")
            })
            .try_for_each(|package| RegistryPackage::new(package)?.install(self, downloads))
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    fs::create_dir_all(path.parent().context("Missing registry directory")?)?;
    let temporary = path.with_extension("partial");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)?;
    Ok(())
}

struct FetchDependencies;

impl FetchDependencies {
    fn into_command(self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("/usr/local/cargo/bin/cargo");
        command.args(["fetch", "--offline"]);
        command
    }
}

enum Resolution {
    Seed,
    Resolve,
    Fetch { request: RegistryRequest },
    Complete,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum RegistryRequest {
    Packages,
    Index { index: RegistryIndex },
}

impl RegistryRequest {
    fn from_diagnostic(stderr: &str) -> anyhow::Result<Option<Self>> {
        match RegistryIndex::from_diagnostic(stderr)? {
            Some(index) => Ok(Some(Self::Index { index })),
            None if stderr.contains("failed to download `")
                && stderr.contains(
                    "attempting to make an HTTP request, but --offline was specified",
                ) =>
            {
                Ok(Some(Self::Packages))
            }
            None => Ok(None),
        }
    }
}

struct DependencyResolver {
    sandbox: Sandbox,
    cancellations: Vec<CancellationToken>,
    requested: HashSet<RegistryRequest>,
}

impl DependencyResolver {
    async fn run(mut self) -> anyhow::Result<()> {
        let mut state = Resolution::Seed;
        while !matches!(state, Resolution::Complete) {
            state = match state {
                Resolution::Seed => {
                    self.fetch(RegistryRequest::Packages).await?;
                    Resolution::Resolve
                }
                Resolution::Resolve => self.resolve().await?,
                Resolution::Fetch { request } => {
                    self.fetch(request).await?;
                    Resolution::Resolve
                }
                Resolution::Complete => Resolution::Complete,
            };
        }
        Ok(())
    }

    async fn resolve(&mut self) -> anyhow::Result<Resolution> {
        let result = tokio::select! {
            result = self.sandbox.capture(FetchDependencies.into_command(), ProcessLimits::new(Duration::from_secs(30), 16 * 1024 * 1024)?, self.cancellations.clone()) => result?,
            _ = futures::future::select_all(self.cancellations.iter().map(|token| Box::pin(token.cancelled()))) => Err(anyhow::anyhow!("Process cancelled before launch"))?,
        };
        match result.exit_code {
            Some(0) => Ok(Resolution::Complete),
            _ => match RegistryRequest::from_diagnostic(&result.stderr)? {
                Some(request)
                    if self.requested.len() < 2048 && self.requested.insert(request.clone()) =>
                {
                    Ok(Resolution::Fetch { request })
                }
                _ => Ok(Resolution::Complete),
            },
        }
    }

    async fn fetch(&self, request: RegistryRequest) -> anyhow::Result<()> {
        let workspace = self.sandbox.workspace.clone();
        let cancellations = self.cancellations.clone();
        self.sandbox
            .tasks
            .spawn_blocking(move || {
                let check = || match cancellations.iter().any(|cancel| cancel.is_cancelled()) {
                    true => Err(anyhow::anyhow!("Process cancelled before launch")),
                    false => Ok(()),
                };
                let runtime = super::bootstrap::Installation::new(workspace.as_ref(), &check)?;
                let installation = super::provision::download::Installation::new(
                    super::provision::cache()?,
                    &check,
                )?;
                let downloads = Downloads::new(installation.path().join("downloads"), &check)?;
                let registry = RegistryCache::new(&runtime.rootfs)?;
                match request {
                    RegistryRequest::Packages => {
                        registry.install_packages(workspace.as_ref(), &downloads)
                    }
                    RegistryRequest::Index { index } => {
                        registry.install_index(&index, None, &downloads)
                    }
                }
            })
            .await?
    }
}

pub(crate) fn prepare(
    sandbox: Sandbox,
    cancellations: Vec<CancellationToken>,
) -> futures::future::BoxFuture<'static, anyhow::Result<()>> {
    Box::pin(
        DependencyResolver {
            sandbox,
            cancellations,
            requested: HashSet::new(),
        }
        .run(),
    )
}
#[cfg(test)]
#[path = "../../tests/unit/isolation/registry/tests.rs"]
mod tests;
