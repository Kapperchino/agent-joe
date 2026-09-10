mod krun;
#[path = "../../src/protocol.rs"]
mod protocol;

fn main() -> anyhow::Result<()> {
    krun::run()
}
