use crate::tool_defs::{ToolDefTrait, ToolId, ToolTrait, ToolType};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::cargo;
use utils::cargo::Cargo;

#[async_trait]
impl<C: Context, A> ToolTrait<C, A> for CargoTest {
    type Input = CargoTestInput;
    type Output = CargoTestToolResult;

    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        _cur_context: &C,
        _actor_context: &A,
    ) -> anyhow::Result<Self::Output> {
        let result = CargoTest {
            input,
            id: String::new(),
        }
        .cargo_test()
        .await?;

        let status = match result {
            CargoTestResult::Success { .. } => "success",
            CargoTestResult::Failed { .. } => "failed",
        }
        .to_string();

        Ok(CargoTestToolResult {
            status,
            result,
            id: tool_id,
        })
    }

    fn display_input(input: &Self::Input) -> String {
        CargoTest {
            input: input.clone(),
            id: String::new(),
        }
        .to_string()
    }

    fn req_from_input(
        input: &Self::Input,
    ) -> anyhow::Result<std::collections::HashMap<String, String>> {
        CargoTest {
            input: input.clone(),
            id: String::new(),
        }
        .req()
    }

    fn output_to_content(_input: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        let test_output = match &output.result {
            CargoTestResult::Success { output } | CargoTestResult::Failed { output } => output,
        };
        if test_output.trim().is_empty() {
            Ok(output.status.clone())
        } else {
            Ok(format!("{}\n{}", output.status, test_output))
        }
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "cargo_test",
    description = "Run cargo test on the project to find failing tests, do not enable warning unless explicitly asked to"
)]
pub struct CargoTest {
    #[tool(input)]
    pub input: CargoTestInput,
    pub id: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct CargoTestInput {
    #[serde(default)]
    #[tool(description = "Optional package/workspace member to run tests in")]
    pub package: Option<String>,
    #[serde(default)]
    #[tool(description = "Optional test name/filter to run. Empty runs all tests")]
    pub test_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CargoTestResult {
    Success { output: String },
    Failed { output: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoTestToolResult {
    pub status: String,
    pub result: CargoTestResult,
    pub id: ToolId,
}

impl Display for CargoTest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(test_name) = self
            .input
            .test_name
            .as_ref()
            .filter(|name| !name.trim().is_empty())
        {
            write!(f, "- run `cargo test {}`", test_name)
        } else {
            write!(f, "- run `cargo test`")
        }
    }
}

impl CargoTest {
    async fn cargo_test(&self) -> anyhow::Result<CargoTestResult> {
        let res = Cargo::cargo_test(
            self.input.package.as_deref(),
            self.input.test_name.as_deref(),
        )
        .await?;
        match res {
            cargo::CargoTest::TestPasses { output } => Ok(CargoTestResult::Success { output }),
            cargo::CargoTest::TestFailed { output } => Ok(CargoTestResult::Failed { output }),
        }
    }
}
