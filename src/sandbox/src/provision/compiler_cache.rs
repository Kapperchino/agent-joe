use super::{artifact::Artifact, download::Downloads, platform::Architecture};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

pub(super) const VERSION: &str = "0.18.0";

pub(super) fn install(
    downloads: &Downloads<'_>,
    architecture: Architecture,
    staging: &Path,
) -> anyhow::Result<()> {
    let checksum = match architecture {
        Architecture::Arm64 => "2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a",
        Architecture::Amd64 => "45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89",
    };
    let name = format!(
        "sccache-v{VERSION}-{}-unknown-linux-musl",
        architecture.rust()
    );
    let artifact = Artifact::new(
        &format!("https://github.com/mozilla/sccache/releases/download/v{VERSION}/{name}.tar.gz"),
        checksum,
    )?;
    let archive = downloads.get(&artifact, None)?;
    let unpacked = staging.join("compiler-cache");
    downloads.unpack(&archive, &unpacked)?;
    let destination = staging.join("rootfs/usr/local/bin/sccache");
    fs::create_dir_all(staging.join("rootfs/usr/local/bin"))?;
    fs::copy(unpacked.join(name).join("sccache"), &destination)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;
    fs::remove_dir_all(unpacked)?;
    Ok(())
}
