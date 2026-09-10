#[cfg(unix)]
#[path = "src/provision/native.rs"]
mod native;
#[cfg(unix)]
#[path = "src/provision/mod.rs"]
mod provision;

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=src/provision");
    println!("cargo:rerun-if-changed=launcher/src");
    println!("cargo:rerun-if-changed=launcher/Cargo.toml");
    println!("cargo:rerun-if-changed=launcher/Cargo.lock");
    println!("cargo:rerun-if-changed=guest.sh");
    println!("cargo:rerun-if-changed=launcher/src/krun.rs");
    println!("cargo:rerun-if-changed=src/protocol.rs");
    #[cfg(unix)]
    {
        let build = native::NativeBuild::new()?;
        let check = || Ok(());
        let installation = provision::download::Installation::new(provision::cache()?, &check)?;
        let downloads =
            provision::download::Downloads::new(installation.path().join("downloads"), &check)?;
        let bundle = build.bundle(&installation, &downloads)?;
        println!("cargo:rustc-env=JOE_SANDBOX_BUNDLE={}", bundle.display());
    }
    Ok(())
}
