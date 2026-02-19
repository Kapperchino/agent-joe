use anyhow::anyhow;
use cargo_metadata::{CompilerMessage, Message};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

struct Cargo {}

pub enum CargoResult {
    CargoCheck(CargoCheck),
    CargoTest(CargoTest),
}

pub enum CargoCheck {
    CheckPasses(Vec<CompilerMessage>),
    CheckFailed(Vec<CompilerMessage>),
}

pub enum CargoTest {}

impl Cargo {
    pub async fn cargo_check() -> anyhow::Result<()> {
        let mut child = Command::new("cargo")
            .args(["check", "--message-format=json-diagnostic-short"])
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or(anyhow!("no std output"))?;
        let mut reader = BufReader::new(stdout).lines();
        while let Some(line) = reader.next_line().await? {
            let message: Message = serde_json::from_str(&line)?;
            match message {
                Message::CompilerMessage(msg) => {
                    println!("{:?}", msg.message);
                }
                Message::CompilerArtifact(artifact) => {
                    println!("Built: {}", artifact.target.name);
                }
                Message::BuildFinished(finished) => {
                    println!("Success: {}", finished.success);
                }
                _ => {}
            }
        }

        let status = child.wait().await?;
        println!("Exit code: {}", status);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn temp_file() {
        Cargo::cargo_check().await.unwrap();
    }
}
