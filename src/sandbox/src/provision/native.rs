use crate::provision::{
    artifact::Artifact,
    download::{Downloads, Installation},
    image,
    platform::{Architecture, Platform},
};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct BuildTarget {
    triple: String,
    platform: Platform,
}

impl BuildTarget {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            triple: env!("JOE_SANDBOX_TARGET").into(),
            platform: Platform::current()?,
        })
    }
}

struct LauncherFile {
    path: &'static str,
    contents: &'static [u8],
}

const LAUNCHER_FILES: &[LauncherFile] = &[
    LauncherFile {
        path: "launcher/Cargo.toml",
        contents: include_bytes!("../../launcher/Cargo.toml"),
    },
    LauncherFile {
        path: "launcher/Cargo.lock",
        contents: include_bytes!("../../launcher/Cargo.lock"),
    },
    LauncherFile {
        path: "launcher/src/main.rs",
        contents: include_bytes!("../../launcher/src/main.rs"),
    },
    LauncherFile {
        path: "launcher/src/krun.rs",
        contents: include_bytes!("../../launcher/src/krun.rs"),
    },
    LauncherFile {
        path: "src/protocol.rs",
        contents: include_bytes!("../protocol.rs"),
    },
    LauncherFile {
        path: "entitlements.plist",
        contents: include_bytes!("../../entitlements.plist"),
    },
];

pub struct NativeBuild {
    target: BuildTarget,
}

impl NativeBuild {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            target: BuildTarget::new()?,
        })
    }

    fn version(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(include_bytes!("native.rs"));
        for file in LAUNCHER_FILES {
            digest.update(file.path);
            digest.update(file.contents);
        }
        format!("launcher-{}-{:x}", self.target.triple, digest.finalize())
    }

    pub fn prepare(
        &self,
        installation: &Installation,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<PathBuf> {
        installation.prepare(&self.version(), |staging| {
            eprintln!("Joe is preparing its sandbox launcher for the first time");
            let native = installation.prepare(
                &format!(
                    "launcher-components-1.18.0-fw5.5.0-bwrap0.11.0-{}-v2",
                    self.target.triple
                ),
                |staging| self.prepare_components(staging, installation, downloads),
            )?;
            let source = staging.join("source");
            for file in LAUNCHER_FILES {
                let path = source.join(file.path);
                fs::create_dir_all(path.parent().context("Missing launcher source directory")?)?;
                fs::write(path, file.contents)?;
            }
            let output = installation
                .path()
                .join(format!("launcher-target-{}", self.target.triple));
            downloads.check()?;
            run(self
                .cargo(&output)
                .current_dir(source.join("launcher"))
                .env("KRUN_INIT_BINARY_PATH", native.join("joe-init"))
                .env("KRUN_EDK2_BINARY_PATH", native.join("KRUN_EFI.silent.fd")))?;
            downloads.check()?;
            fs::create_dir(staging.join("lib"))?;
            fs::create_dir(staging.join("bin"))?;
            fs::copy(
                native.join("lib").join(self.target.platform.firmware()),
                staging.join("lib").join(self.target.platform.firmware()),
            )?;
            let helper = staging.join("bin/joe-sandbox");
            fs::copy(
                output
                    .join(&self.target.triple)
                    .join("release/joe-sandbox-launcher"),
                &helper,
            )?;
            match self.target.platform {
                Platform::MacOs => run(Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-", "--entitlements"])
                    .arg(source.join("entitlements.plist"))
                    .arg(helper))?,
                Platform::Linux { .. } => {
                    fs::copy(native.join("bwrap"), staging.join("bin/bwrap"))?;
                }
            }
            fs::remove_dir_all(source)?;
            Ok(())
        })
    }

    fn cargo(&self, directory: &Path) -> Command {
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command
            .args(["build", "--release", "--locked", "--target"])
            .arg(&self.target.triple)
            .arg("--target-dir")
            .arg(directory)
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("CARGO_MAKEFLAGS");
        command
    }

    fn prepare_components(
        &self,
        staging: &Path,
        installation: &Installation,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<()> {
        eprintln!(
            "Joe is preparing its sandbox launcher components for {}",
            self.target.triple
        );
        let artifact = Artifact::new(
            "https://github.com/libkrun/libkrun/archive/refs/tags/v1.18.0.tar.gz",
            "3aad8087049c77424b2675ba08fe7b53708000e6df242d606e45af731f8a62cd",
        )?;
        let archive = downloads.get(&artifact, None)?;
        let build = staging.join("build");
        downloads.unpack(&archive, &build)?;
        let source = build.join("libkrun-1.18.0");
        let library = staging.join("lib");
        fs::create_dir(&library)?;
        let init = staging.join("joe-init");
        run(self
            .init_compiler(installation, downloads)?
            .args(["-O2", "-static", "-Wl,-strip-debug"])
            .arg(source.join("init/init.c"))
            .arg(source.join("init/dhcp.c"))
            .arg("-o")
            .arg(&init))?;
        if let Architecture::Arm64 = self.target.platform.architecture() {
            fs::copy(
                source.join("edk2/KRUN_EFI.silent.fd"),
                staging.join("KRUN_EFI.silent.fd"),
            )?;
        }
        self.compile_firmware(&build, &library, downloads)?;
        if let Platform::Linux { .. } = self.target.platform {
            build_bubblewrap(downloads, &build, &staging.join("bwrap"))?;
        }
        fs::remove_dir_all(build)?;
        Ok(())
    }

    fn init_compiler(
        &self,
        installation: &Installation,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<Command> {
        match self.target.platform {
            Platform::Linux { .. } => Ok(Command::new("cc")),
            Platform::MacOs => {
                let rootfs =
                    image::prepare(installation, downloads, self.target.platform.architecture())?;
                let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
                let rustlib = Command::new(rustc)
                    .args(["--print", "target-libdir"])
                    .output()?;
                let rustlib = PathBuf::from(String::from_utf8(rustlib.stdout)?.trim());
                let linker = rustlib
                    .parent()
                    .context("Missing Rust toolchain binaries")?
                    .join("bin/gcc-ld/ld.lld");
                let gcc = rootfs.join("usr/lib/gcc/aarch64-linux-gnu/12");
                let mut compiler = Command::new("/usr/bin/clang");
                compiler
                    .args(["-target", "aarch64-linux-gnu", "-Wno-c23-extensions"])
                    .arg(format!("-fuse-ld={}", linker.display()))
                    .arg("--sysroot")
                    .arg(rootfs)
                    .arg("-B")
                    .arg(&gcc)
                    .arg("-L")
                    .arg(gcc);
                Ok(compiler)
            }
        }
    }

    fn compile_firmware(
        &self,
        build: &Path,
        library: &Path,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<()> {
        let artifact = match self.target.platform {
            Platform::MacOs => Artifact::new(
                "https://github.com/libkrun/libkrunfw/releases/download/v5.5.0/libkrunfw-prebuilt-aarch64.tgz",
                "5bfae6efee63dbdf04a8fac2a69d772d9f900af2f54c4429b4acdfd6d86b9979",
            ),
            Platform::Linux {
                architecture: Architecture::Arm64,
            } => Artifact::new(
                "https://github.com/libkrun/libkrunfw/releases/download/v5.5.0/libkrunfw-aarch64.tgz",
                "b04c9a5520a1ea52b5b35d87559566872246145961c4b6978034c9b9be54b89b",
            ),
            Platform::Linux {
                architecture: Architecture::Amd64,
            } => Artifact::new(
                "https://github.com/libkrun/libkrunfw/releases/download/v5.5.0/libkrunfw-x86_64.tgz",
                "c169206b01c89fbe134f1728bf4f988702bc7f73b4cf73e6fdece447d6fceca1",
            ),
        }?;
        let archive = downloads.get(&artifact, None)?;
        let source = build.join("firmware");
        downloads.unpack(&archive, &source)?;
        let destination = library.join(self.target.platform.firmware());
        match self.target.platform {
            Platform::MacOs => run(Command::new("cc")
                .args([
                    "-fPIC",
                    "-DABI_VERSION=5",
                    "-shared",
                    "-Wl,-install_name,libkrunfw.5.dylib",
                    "-o",
                ])
                .arg(destination)
                .arg(source.join("libkrunfw/kernel.c"))),
            Platform::Linux { .. } => {
                fs::copy(
                    find_file(&source, "libkrunfw.so.5")?
                        .context("Missing native sandbox firmware")?,
                    destination,
                )?;
                Ok(())
            }
        }
    }
}

fn run(command: &mut Command) -> anyhow::Result<()> {
    let output = command
        .output()
        .with_context(|| format!("Cannot execute {command:?}"))?;
    match output.status.success() {
        true => Ok(()),
        false => Err(anyhow::anyhow!(
            "{command:?} failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

fn find_file(directory: &Path, name: &str) -> anyhow::Result<Option<PathBuf>> {
    fs::read_dir(directory)?.try_fold(None, |found, entry| {
        let entry = entry?;
        match found {
            Some(_) => Ok(found),
            None if entry.file_name() == name => Ok(Some(entry.path())),
            None if entry.file_type()?.is_dir() => find_file(&entry.path(), name),
            None => Ok(None),
        }
    })
}

fn build_bubblewrap(downloads: &Downloads<'_>, build: &Path, output: &Path) -> anyhow::Result<()> {
    let source = Artifact::new(
        "https://github.com/containers/bubblewrap/archive/refs/tags/v0.11.0.tar.gz",
        "cfeeb15fcc47d177d195f06fdf0847e93ee3aa6bf46f6ac0a141fa142759e2c3",
    )?;
    downloads.unpack(&downloads.get(&source, None)?, build)?;
    let libcap = Artifact::new(
        "https://www.kernel.org/pub/linux/libs/security/linux-privs/libcap2/libcap-2.78.tar.gz",
        "2a2c705e382c413643a458b837575c0eb0989477ab6fb99c87adbe9a259612ad",
    )?;
    downloads.unpack(&downloads.get(&libcap, None)?, build)?;
    let libcap = build.join("libcap-2.78/libcap");
    run(Command::new("make").current_dir(&libcap).args([
        "libcap.a",
        "USE_GPERF=no",
        "PTHREADS=no",
        "SHARED=no",
    ]))?;
    let source = build.join("bubblewrap-0.11.0");
    fs::write(
        source.join("config.h"),
        "#define PACKAGE_STRING \"bubblewrap 0.11.0\"\n#define ENABLE_REQUIRE_USERNS 1\n",
    )?;
    run(Command::new("cc")
        .current_dir(source)
        .args(["-O2", "-D_GNU_SOURCE", "-I.", "-I"])
        .arg(libcap.join("include"))
        .args(["bubblewrap.c", "bind-mount.c", "network.c", "utils.c"])
        .arg(libcap.join("libcap.a"))
        .arg("-o")
        .arg(output))
}
