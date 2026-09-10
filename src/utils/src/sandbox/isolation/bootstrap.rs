use super::provision;
use crate::workspace::WorkspacePolicy;
#[cfg(target_os = "macos")]
use anyhow::Context;
use sha2::{Digest, Sha256};
#[cfg(target_os = "macos")]
use std::fs;
use std::path::PathBuf;

pub(super) struct Installation {
    pub rootfs: PathBuf,
    pub native: PathBuf,
}

impl Installation {
    pub fn new(
        workspace: &WorkspacePolicy,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let cache = provision::cache()?;
        let directory = provision::directory::PrivateDirectory::new(cache)?;
        let cache = directory.path().to_path_buf();
        let cache =
            match cache.starts_with(workspace.root()) || workspace.root().starts_with(&cache) {
                true => Err(anyhow::anyhow!(
                    "Joe's sandbox cache overlaps the workspace"
                )),
                false => Ok(cache),
            }?;
        let installation = provision::download::Installation::new(cache, check)?;
        let downloads =
            provision::download::Downloads::new(installation.path().join("downloads"), check)?;
        let architecture = provision::platform::Platform::current()?.architecture();
        let rootfs = provision::image::prepare(&installation, &downloads, architecture)?;
        let bundle = include_bytes!(env!("JOE_SANDBOX_BUNDLE"));
        let version = format!("launcher-{:x}", Sha256::digest(bundle));
        let native = installation.prepare(&version, |staging| {
            let archive = flate2::read::GzDecoder::new(bundle.as_slice());
            tar::Archive::new(archive).unpack(staging)?;
            #[cfg(target_os = "macos")]
            {
                let entitlements = staging.join("entitlements.plist");
                fs::write(
                    &entitlements,
                    include_bytes!("../../../../../sandbox/entitlements.plist"),
                )?;
                let output = std::process::Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-", "--entitlements"])
                    .arg(entitlements)
                    .arg(staging.join("bin/joe-sandbox"))
                    .output()
                    .context("Cannot sign Joe's sandbox launcher")?;
                match output.status.success() {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!(
                        "Cannot sign Joe's sandbox launcher: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )),
                }?;
            }
            Ok(())
        })?;
        check()?;
        Ok(Self { rootfs, native })
    }
}
