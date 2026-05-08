use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::cargo;
use utils::cargo::Cargo;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for CargoCheck {
    type Input = CargoCheckInput;
    type Output = CargoCheckToolResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let result = CargoCheck {
            input,
            id: String::new(),
        }
        .cargo_check()
        .await?;

        let status = match result {
            CargoCheckResult::Success(_) => "success",
            CargoCheckResult::Failed { .. } => "failed",
        }
        .to_string();

        Ok(CargoCheckToolResult {
            status,
            result,
            id: tool_id,
        })
    }

    fn display_input(input: &Self::Input) -> String {
        CargoCheck {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        CargoCheck {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        match &output.result {
            CargoCheckResult::Success(warnings) => {
                if input.include_warnings.unwrap_or(false) {
                    Ok(format!(
                        "{}\nWarnings:\n{}",
                        output.status,
                        warnings.join("\n")
                    ))
                } else {
                    Ok(output.status.clone())
                }
            }
            CargoCheckResult::Failed { warnings, errors } => {
                if input.include_warnings.unwrap_or(false) {
                    Ok(format!(
                        "{}\nWarnings:\n{}\nErrors:\n{}",
                        output.status,
                        warnings.join("\n"),
                        errors.join("\n")
                    ))
                } else {
                    Ok(format!("{}\nErrors:\n{}", output.status, errors.join("\n")))
                }
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "cargo_check",
    description = "Run cargo check on the project to find compilation errors and warnings"
)]
pub struct CargoCheck {
    #[tool(input)]
    pub input: CargoCheckInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct CargoCheckInput {
    #[serde(default)]
    #[tool(description = "Include warnings in the output, defaults to false")]
    pub include_warnings: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CargoCheckResult {
    Success(Vec<String>),
    Failed {
        warnings: Vec<String>,
        errors: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoCheckToolResult {
    pub status: String,
    pub result: CargoCheckResult,
    pub id: ToolId,
}

impl Display for CargoCheck {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.input.include_warnings.unwrap_or(false) {
            write!(f, "- run `cargo check` (with warnings)")
        } else {
            write!(f, "- run `cargo check`")
        }
    }
}

impl CargoCheck {
    async fn cargo_check(&self) -> anyhow::Result<CargoCheckResult> {
        let res = Cargo::cargo_check().await?;
        match res {
            cargo::CargoCheck::CheckPasses { warnings } => {
                let warnings = if self.input.include_warnings.unwrap_or(false) {
                    warnings
                        .into_iter()
                        .map(|warning| warning.message.to_string())
                        .collect()
                } else {
                    vec![]
                };
                Ok(CargoCheckResult::Success(warnings))
            }
            cargo::CargoCheck::CheckFailed { failures, warnings } => {
                let warnings = if self.input.include_warnings.unwrap_or(false) {
                    warnings
                        .into_iter()
                        .map(|warning| warning.message.to_string())
                        .collect()
                } else {
                    vec![]
                };
                let errors = failures
                    .into_iter()
                    .map(|failure| failure.message.to_string())
                    .collect();
                Ok(CargoCheckResult::Failed { warnings, errors })
            }
        }
    }
}
