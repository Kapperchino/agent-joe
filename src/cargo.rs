use anyhow::anyhow;
use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::{CompilerMessage, Message};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub struct Cargo {}

pub enum CargoCheck {
    CheckPasses {
        warnings: Vec<CompilerMessage>,
    },
    CheckFailed {
        failures: Vec<CompilerMessage>,
        warnings: Vec<CompilerMessage>,
    },
}

pub enum CargoTest {}

impl Cargo {
    pub async fn cargo_check() -> anyhow::Result<CargoCheck> {
        let mut child = Command::new("cargo")
            .args(["check", "--message-format=json-diagnostic-short"])
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().ok_or(anyhow!("no std output"))?;
        let mut reader = BufReader::new(stdout).lines();
        let mut warnings = Vec::new();
        let mut failures = Vec::new();
        let mut success = false;
        while let Some(line) = reader.next_line().await? {
            let message: Message = serde_json::from_str(&line)?;
            match message {
                Message::CompilerMessage(msg) => match msg.message.level {
                    DiagnosticLevel::Warning => {
                        warnings.push(msg);
                    }
                    DiagnosticLevel::Error => {
                        failures.push(msg);
                    }
                    _ => {}
                },
                Message::BuildFinished(finished) => {
                    success = finished.success;
                }
                _ => {}
            }
        }

        child.wait().await?;
        match success {
            true => Ok(CargoCheck::CheckPasses { warnings }),
            false => Ok(CargoCheck::CheckFailed { failures, warnings }),
        }
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
