use anyhow::anyhow;
use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::{CompilerMessage, Message};
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
        let mut command = Command::new("cargo");
        command.args(["check", "--message-format=json-diagnostic-short"]);
        crate::process::output(command).await.and_then(|output| {
            String::from_utf8_lossy(&output.stdout).lines()
                .map(serde_json::from_str::<Message>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(anyhow::Error::from)
                .and_then(|messages| {
                    let success = output.status.success() && messages.iter()
                        .any(|message| matches!(message, Message::BuildFinished(finished) if finished.success));
                    let mut warnings = Vec::new();
                    let mut failures = Vec::new();
                    for message in messages {
                        if let Message::CompilerMessage(message) = message {
                            match message.message.level {
                                DiagnosticLevel::Warning => warnings.push(message),
                                DiagnosticLevel::Error => failures.push(message),
                                _ => {},
                            }
                        }
                    }
                    if !output.status.success() && failures.is_empty() {
                        Err(anyhow!("Cargo check failed: {}", String::from_utf8_lossy(&output.stderr)))
                    } else if success {
                        Ok(CargoCheck::CheckPasses { warnings })
                    } else {
                        Ok(CargoCheck::CheckFailed { failures, warnings })
                    }
                })
        })
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
        crate::process::output(command).await.map(|result| {
            let status = result.status;
            let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&result.stderr).into_owned();

            let output = if stderr.trim().is_empty() {
                stdout
            } else if stdout.trim().is_empty() {
                stderr
            } else {
                format!("STDOUT:\n{}\n\nSTDERR:\n{}", stdout, stderr)
            };

            if status.success() {
                CargoTest::TestPasses { output }
            } else {
                CargoTest::TestFailed { output }
            }
        })
    }
}
