use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;
use std::marker::PhantomData;
use std::sync::Arc;
use turbo_code_macros::ToolInput;

pub trait ToolDefTrait {
    fn tool_name() -> &'static str;
    fn tool_description() -> &'static str;
    fn field_properties() -> HashMap<String, ToolProperty>;
    fn required_fields() -> Vec<String>;
    fn req(&self) -> anyhow::Result<HashMap<String, String>>;
}

pub trait ToolInputSchema {
    fn properties() -> HashMap<String, ToolProperty>;
    fn required() -> Vec<String>;
    fn req(&self) -> anyhow::Result<HashMap<String, String>>;
}

pub trait ToolUse {}

#[async_trait]
pub trait ToolTrait: ToolDefTrait + Display {
    type Input;
    type Output;
    async fn run<C: Context>(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
    ) -> anyhow::Result<Self::Output>;

    fn display_input(input: &Self::Input) -> String;

    fn req_from_input(input: &Self::Input) -> anyhow::Result<HashMap<String, String>>;

    fn output_to_content(input: &Self::Input, output: &Self::Output) -> anyhow::Result<String>;

    fn name(&self) -> String {
        Self::tool_name().to_string()
    }

    fn to_req(&self) -> anyhow::Result<HashMap<String, String>> {
        self.req()
    }
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub properties: HashMap<String, ToolProperty>,
    pub required: Vec<String>,
}

#[async_trait]
pub trait ErasedToolTrait<C: Context>: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    fn name(&self) -> String {
        self.definition().name
    }

    fn display_erased(&self, input: &Value) -> anyhow::Result<String>;

    fn input_req_erased(&self, input: &Value) -> anyhow::Result<HashMap<String, String>>;

    async fn run_erased(
        &self,
        input: Value,
        tool_id: ToolId,
        cur_context: &C,
    ) -> anyhow::Result<Value>;

    fn output_to_content_erased(&self, input: &Value, output: &Value) -> anyhow::Result<String>;
}

pub struct ErasedTool<T, C> {
    _marker: PhantomData<T>,
    _context: PhantomData<C>,
}

impl<T, C> ErasedTool<T, C> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
            _context: PhantomData,
        }
    }
}

#[async_trait]
impl<T, C> ErasedToolTrait<C> for ErasedTool<T, C>
where
    T: ToolTrait + Send + Sync,
    T::Input: Clone + DeserializeOwned + Send,
    T::Output: DeserializeOwned + Serialize + Send,
    C: Send + Sync + Context,
{
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: T::tool_name().to_string(),
            description: T::tool_description().to_string(),
            properties: T::field_properties(),
            required: T::required_fields(),
        }
    }

    fn display_erased(&self, input: &Value) -> anyhow::Result<String> {
        let typed_input: T::Input = serde_json::from_value(input.clone())?;
        Ok(T::display_input(&typed_input))
    }

    fn input_req_erased(&self, input: &Value) -> anyhow::Result<HashMap<String, String>> {
        let typed_input: T::Input = serde_json::from_value(input.clone())?;
        T::req_from_input(&typed_input)
    }

    async fn run_erased(
        &self,
        input: Value,
        tool_id: ToolId,
        cur_context: &C,
    ) -> anyhow::Result<Value> {
        let typed_input: T::Input = serde_json::from_value(input)?;

        let typed_output: T::Output = T::run::<C>(typed_input, tool_id, cur_context).await?;

        let erased_output = serde_json::to_value(typed_output)?;

        Ok(erased_output)
    }

    fn output_to_content_erased(&self, input: &Value, output: &Value) -> anyhow::Result<String> {
        let typed_input: T::Input = serde_json::from_value(input.clone())?;
        let typed_output: T::Output = serde_json::from_value(output.clone())?;
        T::output_to_content(&typed_input, &typed_output)
    }
}

pub type ErasedToolRef<C> = Arc<dyn ErasedToolTrait<C>>;

pub fn erased_tool<T, C>() -> ErasedToolRef<C>
where
    T: ToolTrait + Send + Sync + 'static,
    T::Input: Clone + DeserializeOwned + Send + 'static,
    T::Output: DeserializeOwned + Serialize + Send + 'static,
    C: Send + Sync + Context + 'static,
{
    Arc::new(ErasedTool::<T, C>::new())
}

pub trait LenientDeserialize: Sized {
    fn deserialize_lenient(s: &str) -> anyhow::Result<Self>;
}

#[derive(Debug, Clone)]
pub enum ToolProperty {
    Value {
        name: String,
        prop_type: String,
        description: String,
    },
    Object {
        name: String,
        prop_type: String,
        description: String,
        properties: HashMap<String, ToolProperty>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Eq, PartialEq, Hash, ToolInput)]
pub struct Range {
    #[tool(description = "Start line (inclusive)", required)]
    pub start: u32,
    #[tool(description = "End line (exclusive)", required)]
    pub end: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolId {
    pub call_id: Option<String>,
    pub id: String,
}

#[derive(Debug)]
pub struct ToolInvocation {
    pub name: String,
    pub input: HashMap<String, String>,
    pub display: String,
}

#[derive(Debug)]
pub struct ToolResult {
    pub id: ToolId,
    pub invocation: ToolInvocation,
    pub content: String,
    pub is_error: bool,
}
