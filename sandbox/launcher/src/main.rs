#[path = "../../../src/utils/src/sandbox/krun.rs"]
mod krun;
#[path = "../../../src/utils/src/sandbox/protocol.rs"]
mod protocol;

fn main() -> anyhow::Result<()> {
    krun::run()
}
