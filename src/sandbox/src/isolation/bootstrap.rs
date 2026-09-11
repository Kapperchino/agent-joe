use super::provision;
use crate::workspace::Workspace;
use std::path::PathBuf;

pub(super) struct Installation {
    pub rootfs: PathBuf,
    pub native: PathBuf,
}

impl Installation {
    pub fn new(
        workspace: &dyn Workspace,
        check: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let native = provision::native::NativeRuntime::new()?;
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
        let native = native.prepare(&installation, &downloads)?;
        check()?;
        Ok(Self { rootfs, native })
    }
}
