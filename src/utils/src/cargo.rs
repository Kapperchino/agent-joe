use anyhow::anyhow;
use cargo_metadata::diagnostic::DiagnosticLevel;
use cargo_metadata::{CompilerMessage, Message};
use tokio::process::Command;

pub struct Cargo {}

pub struct CargoSelector(String);

impl CargoSelector {
    pub fn new(value: &str) -> anyhow::Result<Self> {
        if value.is_empty()
            || value.starts_with('-')
            || value.chars().any(char::is_whitespace)
            || value.chars().any(char::is_control)
        {
            Err(anyhow!("Invalid Cargo selector: {value:?}"))
        } else {
            Ok(Self(value.to_owned()))
        }
    }
}

pub enum CargoOperation {
    Check,
    Test {
        package: Option<CargoSelector>,
        test_name: Option<CargoSelector>,
    },
}

impl CargoOperation {
    pub fn test(package: Option<&str>, test_name: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self::Test {
            package: package
                .filter(|name| !name.trim().is_empty())
                .map(CargoSelector::new)
                .transpose()?,
            test_name: test_name
                .filter(|name| !name.trim().is_empty())
                .map(CargoSelector::new)
                .transpose()?,
        })
    }
}

impl crate::sandbox::SandboxOperation for CargoOperation {}

impl crate::sandbox::sealed::Operation for CargoOperation {
    fn into_command(self) -> Command {
        let mut command = Command::new("cargo");
        match self {
            Self::Check => {
                command.args([
                    "check",
                    "--offline",
                    "--message-format=json-diagnostic-short",
                ]);
            }
            Self::Test { package, test_name } => {
                command.args(["test", "--offline"]);
                if let Some(package) = package {
                    command.arg("-p").arg(package.0);
                }
                if let Some(test_name) = test_name {
                    command.arg(test_name.0);
                }
            }
        }
        command
    }
}

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
        crate::sandbox::Sandbox::output(CargoOperation::Check).await.and_then(|output| {
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
        crate::sandbox::Sandbox::output(CargoOperation::test(package, test_name)?)
            .await
            .map(|result| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_cannot_inject_cargo_options() {
        for invalid in [
            "--manifest-path=outside/Cargo.toml",
            "--config=net.offline=false",
            "-p",
            "",
            "test\n--release",
        ] {
            assert!(CargoSelector::new(invalid).is_err());
        }
        for valid in ["workspace-package", "module::test", "test_filter"] {
            assert!(CargoSelector::new(valid).is_ok());
        }
    }
}
