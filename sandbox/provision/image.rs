use super::{
    artifact::{ArchivePath, Artifact},
    download::{Downloads, Installation},
    platform::Architecture,
};
use anyhow::Context;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

impl Architecture {
    fn manifest(self) -> &'static str {
        match self {
            Self::Arm64 => "8e45ae5b178fa788bbbd818b42a1f93a6e2c03e7144badd5e0a37087537177e1",
            Self::Amd64 => "4c2fd73ef19c5ef9d54bee03b06b2839a392604fbfcd578ed948b71b37c1d7fb",
        }
    }
}

#[derive(Clone, Copy)]
enum RustComponent {
    Formatter,
    Clippy,
}

impl RustComponent {
    fn name(self) -> &'static str {
        match self {
            Self::Formatter => "rustfmt",
            Self::Clippy => "clippy",
        }
    }

    fn checksum(self, architecture: Architecture) -> &'static str {
        match (self, architecture) {
            (Self::Formatter, Architecture::Arm64) => {
                "ee169a16fb2a415aef71fba78fb279b7e0bb875dfecd56cdb80bdb95ba29a559"
            }
            (Self::Clippy, Architecture::Arm64) => {
                "84480abfbfed89616b1e0b35acb9db84766f0353b671766638084f4e9b1d1143"
            }
            (Self::Formatter, Architecture::Amd64) => {
                "6d3e64adc505ad4bef6935f6e6f5e4c6956d9782607fb8394df3a5d2b30d2733"
            }
            (Self::Clippy, Architecture::Amd64) => {
                "5230c92fb0ae1346ee30a1b4e01413cc3e1ead8ded20391fb2826619775f1ccb"
            }
        }
    }

    fn install(
        self,
        downloads: &Downloads<'_>,
        architecture: Architecture,
        staging: &Path,
    ) -> anyhow::Result<()> {
        let name = self.name();
        let component = format!("{name}-1.95.0-{}-unknown-linux-gnu", architecture.rust());
        let artifact = Artifact::new(
            &format!("https://static.rust-lang.org/dist/{component}.tar.gz"),
            self.checksum(architecture),
        )?;
        let archive = downloads.get(&artifact, None)?;
        let components = staging.join("components");
        downloads.unpack(&archive, &components)?;
        let payload = components.join(component).join(format!("{name}-preview"));
        let toolchain = staging.join(format!(
            "rootfs/usr/local/rustup/toolchains/1.95.0-{}-unknown-linux-gnu",
            architecture.rust()
        ));
        for entry in fs::read_to_string(payload.join("manifest.in"))?.lines() {
            let file = ArchivePath::new(
                entry
                    .strip_prefix("file:")
                    .context("Unsupported Rust component entry")?
                    .into(),
            )?;
            let destination = toolchain.join(file.path());
            fs::create_dir_all(destination.parent().context("Missing component parent")?)?;
            fs::copy(payload.join(file.path()), destination)?;
        }
        let installed = toolchain.join("lib/rustlib/components");
        let contents = format!("{}{name}-preview\n", fs::read_to_string(&installed)?);
        fs::write(installed, contents)?;
        fs::remove_dir_all(components)?;
        Ok(())
    }
}

#[derive(Deserialize)]
struct Token {
    token: String,
}

#[derive(Deserialize)]
struct Manifest {
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct Layer {
    digest: String,
}

impl Layer {
    fn artifact(&self) -> anyhow::Result<Artifact> {
        Artifact::new(
            &format!(
                "https://registry-1.docker.io/v2/library/rust/blobs/{}",
                self.digest
            ),
            self.digest
                .strip_prefix("sha256:")
                .context("Unsupported sandbox layer digest")?,
        )
    }
}

pub fn prepare(
    installation: &Installation,
    downloads: &Downloads<'_>,
    architecture: Architecture,
) -> anyhow::Result<PathBuf> {
    installation.prepare(&format!("guest-{}-v1", architecture.manifest()), |staging| {
        eprintln!("Joe is preparing its Linux sandbox for the first time");
        let token: Token = downloads.json("https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/rust:pull")?;
        let artifact = Artifact::new(
            &format!("https://registry-1.docker.io/v2/library/rust/manifests/sha256:{}", architecture.manifest()),
            architecture.manifest(),
        )?;
        let manifest = downloads.get(&artifact, Some(&token.token))?;
        let manifest: Manifest = serde_json::from_reader(fs::File::open(manifest)?)?;
        let rootfs = staging.join("rootfs");
        for layer in manifest.layers {
            let archive = downloads.get(&layer.artifact()?, Some(&token.token))?;
            downloads.unpack(&archive, &rootfs)?;
        }
        for component in [RustComponent::Formatter, RustComponent::Clippy] {
            component.install(downloads, architecture, staging)?;
        }
        for directory in ["workspace", "dev", "proc", "sys", "tmp", "usr/local/libexec", "usr/local/cargo/registry/index", "usr/local/cargo/registry/cache"] {
            fs::create_dir_all(rootfs.join(directory))?;
        }
        fs::write(rootfs.join("usr/local/libexec/joe-guest"), include_bytes!("../guest.sh"))?;
        Ok(())
    }).map(|path| path.join("rootfs"))
}
