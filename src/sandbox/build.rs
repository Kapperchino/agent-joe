fn main() -> Result<(), std::env::VarError> {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-env=JOE_SANDBOX_TARGET={}",
        std::env::var("TARGET")?
    );
    Ok(())
}
