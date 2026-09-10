#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
    Amd64,
}

impl Architecture {
    pub fn new(name: &str) -> anyhow::Result<Self> {
        match name {
            "aarch64" => Ok(Self::Arm64),
            "x86_64" => Ok(Self::Amd64),
            _ => Err(anyhow::anyhow!("Unsupported sandbox architecture: {name}")),
        }
    }

    pub fn rust(self) -> &'static str {
        match self {
            Self::Arm64 => "aarch64",
            Self::Amd64 => "x86_64",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Linux { architecture: Architecture },
    MacOs,
}

impl Platform {
    pub fn new(os: &str, architecture: &str) -> anyhow::Result<Self> {
        let architecture = Architecture::new(architecture)?;
        match (os, architecture) {
            ("linux", architecture) => Ok(Self::Linux { architecture }),
            ("macos", Architecture::Arm64) => Ok(Self::MacOs),
            _ => Err(anyhow::anyhow!(
                "The sandbox requires Linux or Apple Silicon macOS"
            )),
        }
    }

    pub fn current() -> anyhow::Result<Self> {
        Self::new(std::env::consts::OS, std::env::consts::ARCH)
    }

    pub fn architecture(self) -> Architecture {
        match self {
            Self::Linux { architecture } => architecture,
            Self::MacOs => Architecture::Arm64,
        }
    }

    pub fn firmware(self) -> &'static str {
        match self {
            Self::Linux { .. } => "libkrunfw.so.5",
            Self::MacOs => "libkrunfw.5.dylib",
        }
    }
}
