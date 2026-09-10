#[cfg(unix)]
#[path = "../../sandbox/provision/native.rs"]
mod native;
#[cfg(unix)]
#[path = "../../sandbox/provision/mod.rs"]
mod provision;

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../../sandbox/provision");
    println!("cargo:rerun-if-changed=../../sandbox/launcher/src");
    println!("cargo:rerun-if-changed=../../sandbox/launcher/Cargo.toml");
    println!("cargo:rerun-if-changed=../../sandbox/launcher/Cargo.lock");
    println!("cargo:rerun-if-changed=../../sandbox/guest.sh");
    println!("cargo:rerun-if-changed=src/sandbox/krun.rs");
    println!("cargo:rerun-if-changed=src/sandbox/protocol.rs");
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
