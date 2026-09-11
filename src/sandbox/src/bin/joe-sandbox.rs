#[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
#[path = "../launcher.rs"]
mod launcher;
#[path = "../protocol.rs"]
mod protocol;

fn main() -> anyhow::Result<()> {
    #[cfg(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64")))]
    {
        launcher::run()
    }
    #[cfg(not(any(target_os = "linux", all(target_os = "macos", target_arch = "aarch64"))))]
    {
        Err(anyhow::anyhow!(
            "The sandbox requires Linux or Apple Silicon macOS"
        ))
    }
}
