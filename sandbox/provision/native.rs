use crate::provision::{
    artifact::Artifact,
    download::{Downloads, Installation},
    image,
    platform::{Architecture, Platform},
};
use anyhow::Context;
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
        let host = std::env::var("HOST")?;
        let triple = std::env::var("TARGET")?;
        match triple == host {
            true => Ok(Self {
                triple,
                platform: Platform::current()?,
            }),
            false => Err(anyhow::anyhow!(
                "Joe's bundled sandbox must be built on its target platform"
            )),
        }
    }
}

pub struct NativeBuild {
    target: BuildTarget,
    repository: PathBuf,
    output: PathBuf,
}

impl NativeBuild {
    pub fn new() -> anyhow::Result<Self> {
        let repository = PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").context("Missing manifest directory")?,
        )
        .join("../..")
        .canonicalize()?;
        let output =
            PathBuf::from(std::env::var_os("OUT_DIR").context("Missing build output directory")?);
        Ok(Self {
            target: BuildTarget::new()?,
            repository,
            output,
        })
    }

    pub fn bundle(
        &self,
        installation: &Installation,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<PathBuf> {
        let native = installation.prepare(
            &format!(
                "native-1.18.0-fw5.5.0-bwrap0.11.0-{}-v2",
                self.target.triple
            ),
            |staging| self.compile_runtime(staging, installation, downloads),
        )?;
        run(self
            .cargo(&self.output.join("launcher-target"))
            .current_dir(self.repository.join("sandbox/launcher")))?;
        let archive = self.output.join("sandbox-native.tar.gz");
        let writer = flate2::write::GzEncoder::new(
            fs::File::create(&archive)?,
            flate2::Compression::default(),
        );
        let mut bundle = tar::Builder::new(writer);
        bundle.append_dir_all("lib", native.join("lib"))?;
        bundle.append_path_with_name(
            self.output
                .join("launcher-target")
                .join(&self.target.triple)
                .join("release/joe-sandbox-launcher"),
            "bin/joe-sandbox",
        )?;
        if let Platform::Linux { .. } = self.target.platform {
            bundle.append_path_with_name(native.join("bwrap"), "bin/bwrap")?;
        }
        bundle.into_inner()?.finish()?.sync_all()?;
        Ok(archive)
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

    fn compile_runtime(
        &self,
        staging: &Path,
        installation: &Installation,
        downloads: &Downloads<'_>,
    ) -> anyhow::Result<()> {
        eprintln!(
            "Joe is building its bundled libkrun runtime for {}",
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
        let init = build.join("joe-init");
        run(self
            .init_compiler(installation, downloads)?
            .args(["-O2", "-static", "-Wl,-strip-debug"])
            .arg(source.join("init/init.c"))
            .arg(source.join("init/dhcp.c"))
            .arg("-o")
            .arg(&init))?;
        let target_directory = build.join("target");
        run(self
            .cargo(&target_directory)
            .current_dir(&source)
            .args(["-p", "libkrun", "--lib"])
            .env("KRUN_INIT_BINARY_PATH", init))?;
        let filename = match self.target.platform {
            Platform::MacOs => "libkrun.dylib",
            Platform::Linux { .. } => "libkrun.so",
        };
        fs::copy(
            target_directory
                .join(&self.target.triple)
                .join("release")
                .join(filename),
            library.join(self.target.platform.library()),
        )?;
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
                let rustc = std::env::var_os("RUSTC").context("Missing Rust compiler")?;
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
        match self.target.platform {
            Platform::MacOs => run(Command::new("cc")
                .args([
                    "-fPIC",
                    "-DABI_VERSION=5",
                    "-shared",
                    "-Wl,-install_name,libkrunfw.5.dylib",
                    "-o",
                ])
                .arg(library.join("libkrunfw.5.dylib"))
                .arg(source.join("libkrunfw/kernel.c"))),
            Platform::Linux { .. } => {
                fs::copy(
                    find_file(&source, "libkrunfw.so.5")?
                        .context("Missing native sandbox firmware")?,
                    library.join("libkrunfw.so.5"),
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
