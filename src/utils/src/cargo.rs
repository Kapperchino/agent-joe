use anyhow::anyhow;
use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::{CompilerMessage, Message};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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

pub enum CargoTest {
    TestPasses { output: String },
    TestFailed { output: String },
}

impl Cargo {
    pub async fn cargo_check() -> anyhow::Result<CargoCheck> {
        let mut child = Command::new("cargo")
            .args(["check", "--message-format=json-diagnostic-short"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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

    pub async fn cargo_test(
        package: Option<&str>,
        test_name: Option<&str>,
    ) -> anyhow::Result<CargoTest> {
        let mut command = Command::new("cargo");
        command.arg("test");
        if let Some(package) = package.filter(|name| !name.trim().is_empty()) {
            command.args(["-p", package]);
        }
        if let Some(test_name) = test_name.filter(|name| !name.trim().is_empty()) {
            command.arg(test_name);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().ok_or(anyhow!("no stdout"))?;
        let mut stderr = child.stderr.take().ok_or(anyhow!("no stderr"))?;

        let stdout_task = tokio::spawn(async move {
            let mut buf = String::new();
            stdout.read_to_string(&mut buf).await?;
            Ok::<String, std::io::Error>(buf)
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            stderr.read_to_string(&mut buf).await?;
            Ok::<String, std::io::Error>(buf)
        });

        let status = child.wait().await?;
        let stdout = stdout_task.await??;
        let stderr = stderr_task.await??;

        let output = if stderr.trim().is_empty() {
            stdout
        } else if stdout.trim().is_empty() {
            stderr
        } else {
            format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr)
        };

        if status.success() {
            Ok(CargoTest::TestPasses { output })
        } else {
            Ok(CargoTest::TestFailed { output })
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
