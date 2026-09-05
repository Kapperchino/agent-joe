use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Display;
use std::marker::PhantomData;
use std::sync::Arc;
use turbo_code_macros::ToolInput;
use utils::utils::FnvHashMap;

pub trait ToolDefTrait {
    fn tool_name() -> &'static str;
    fn tool_description() -> &'static str;
    fn field_properties() -> FnvHashMap<String, ToolProperty>;
    fn required_fields() -> Vec<String>;
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>>;
}

pub trait ToolInputSchema {
    fn properties() -> FnvHashMap<String, ToolProperty>;
    fn required() -> Vec<String>;
    fn req(&self) -> anyhow::Result<FnvHashMap<String, String>>;
}

pub trait ToolUse {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    Read,
    Write,
    Validate,
    DelegateRead,
    DelegateWrite,
    DelegateValidate,
}
impl ToolEffect {
    pub fn concurrent(self) -> bool {
        matches!(self, Self::Read | Self::DelegateRead)
    }
    pub fn delegates(self) -> bool {
        matches!(
            self,
            Self::DelegateRead | Self::DelegateWrite | Self::DelegateValidate
        )
    }
}

#[async_trait]
pub trait ToolTrait<C: Context, A>: ToolDefTrait + Display {
    type Input;
    type Output;
    async fn run(
        input: Self::Input,
        tool_id: ToolId,
        cur_context: &C,
        actor_context: &A,
    ) -> anyhow::Result<Self::Output>;

    fn display_input(input: &Self::Input) -> String;

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>>;

    fn output_to_content(input: &Self::Input, output: &Self::Output) -> anyhow::Result<String>;

    fn output_is_error(_output: &Self::Output) -> bool {
        false
    }

    fn name(&self) -> String {
        Self::tool_name().to_string()
    }

    fn to_req(&self) -> anyhow::Result<FnvHashMap<String, String>> {
        self.req()
    }

    fn add_context(input: &Self::Input, _context: &mut C, _addition: &str) {}

    fn tool_type() -> ToolType;

    fn effect() -> ToolEffect {
        ToolEffect::Write
    }
}

#[derive(Debug, Clone)]
pub enum ToolDefinition {
    Client {
        name: String,
        description: String,
        properties: FnvHashMap<String, ToolProperty>,
        required: Vec<String>,
    },
    Search {
        name: String,
    },
}
pub enum ToolType {
    Client,
    Search,
}
#[async_trait]
pub trait ErasedToolTrait<C: Context, A>: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    fn effect(&self) -> ToolEffect {
        ToolEffect::Write
    }

    fn name(&self) -> String {
        match self.definition() {
            ToolDefinition::Client { name, .. } => name,
            ToolDefinition::Search { name, .. } => name,
        }
    }

    fn display_erased(&self, input: &Value) -> anyhow::Result<String>;

    fn input_req_erased(&self, input: &Value) -> anyhow::Result<FnvHashMap<String, String>>;

    async fn run_erased(
        &self,
        input: Value,
        tool_id: ToolId,
        cur_context: &C,
        actor_context: &A,
    ) -> anyhow::Result<Value>;

    fn output_to_content_erased(&self, input: &Value, output: &Value) -> anyhow::Result<String>;

    fn output_is_error_erased(&self, _output: &Value) -> anyhow::Result<bool> {
        Ok(false)
    }

    fn add_context(&self, input: &Value, context: &mut C, addition: &str) -> anyhow::Result<()>;
}

pub struct ErasedTool<T, C, A> {
    _marker: PhantomData<T>,
    _context: PhantomData<C>,
    _actor_context: PhantomData<A>,
}

impl<T, C, A> ErasedTool<T, C, A> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
            _context: PhantomData,
            _actor_context: PhantomData,
        }
    }
}

#[async_trait]
impl<T, C, A> ErasedToolTrait<C, A> for ErasedTool<T, C, A>
where
    T: ToolTrait<C, A> + Send + Sync,
    T::Input: Clone + DeserializeOwned + Send + LenientDeserialize,
    T::Output: DeserializeOwned + Serialize + Send,
    C: Send + Sync + Context,
    A: Send + Sync,
{
    fn definition(&self) -> ToolDefinition {
        match T::tool_type() {
            ToolType::Client => ToolDefinition::Client {
                name: T::tool_name().to_string(),
                description: T::tool_description().to_string(),
                properties: T::field_properties(),
                required: T::required_fields(),
            },
            ToolType::Search => ToolDefinition::Search {
                name: T::tool_name().to_string(),
            },
        }
    }

    fn effect(&self) -> ToolEffect {
        T::effect()
    }

    fn display_erased(&self, input: &Value) -> anyhow::Result<String> {
        let typed_input: T::Input = T::Input::deserialize_lenient(input.clone())?;
        Ok(T::display_input(&typed_input))
    }

    fn input_req_erased(&self, input: &Value) -> anyhow::Result<FnvHashMap<String, String>> {
        let typed_input: T::Input = T::Input::deserialize_lenient(input.clone())?;
        T::req_from_input(&typed_input)
    }

    async fn run_erased(
        &self,
        input: Value,
        tool_id: ToolId,
        cur_context: &C,
        actor_context: &A,
    ) -> anyhow::Result<Value> {
        let typed_input: T::Input = T::Input::deserialize_lenient(input)?;

        let typed_output: T::Output =
            T::run(typed_input, tool_id, cur_context, actor_context).await?;

        let erased_output = serde_json::to_value(typed_output)?;

        Ok(erased_output)
    }

    fn output_to_content_erased(&self, input: &Value, output: &Value) -> anyhow::Result<String> {
        let typed_input: T::Input = T::Input::deserialize_lenient(input.clone())?;
        let typed_output: T::Output = serde_json::from_value(output.clone())?;
        T::output_to_content(&typed_input, &typed_output)
    }

    fn output_is_error_erased(&self, output: &Value) -> anyhow::Result<bool> {
        let output: T::Output = serde_json::from_value(output.clone())?;
        Ok(T::output_is_error(&output))
    }

    fn add_context(&self, input: &Value, context: &mut C, addition: &str) -> anyhow::Result<()> {
        let typed_input: T::Input = T::Input::deserialize_lenient(input.clone())?;
        Ok(T::add_context(&typed_input, context, addition))
    }
}

pub type ErasedToolRef<C, A> = Arc<dyn ErasedToolTrait<C, A>>;

pub fn erased_tool<T, C, A>() -> ErasedToolRef<C, A>
where
    T: ToolTrait<C, A> + Send + Sync + 'static,
    T::Input: Clone + DeserializeOwned + Send + LenientDeserialize + 'static,
    T::Output: DeserializeOwned + Serialize + Send + 'static,
    C: Send + Sync + Context + 'static,
    A: Send + Sync + 'static,
{
    Arc::new(ErasedTool::<T, C, A>::new())
}

pub trait LenientDeserialize: Sized {
    fn deserialize_lenient(s: Value) -> anyhow::Result<Self>;
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
        properties: FnvHashMap<String, ToolProperty>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Eq, PartialEq, Hash, ToolInput)]
pub struct Range {
    #[tool(description = "Start line (inclusive)", required)]
    pub start: u32,
    #[tool(description = "End line (exclusive)", required)]
    pub end: u32,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NonEmptyString(String);

impl TryFrom<String> for NonEmptyString {
    type Error = anyhow::Error;

    fn try_from(value: String) -> anyhow::Result<Self> {
        if value.is_empty() {
            Err(anyhow::anyhow!("Expected a nonempty string"))
        } else {
            Ok(Self(value))
        }
    }
}

impl From<NonEmptyString> for String {
    fn from(value: NonEmptyString) -> Self {
        value.0
    }
}

impl AsRef<str> for NonEmptyString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for NonEmptyString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolId {
    pub call_id: Option<NonEmptyString>,
    pub id: NonEmptyString,
}

#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub name: NonEmptyString,
    pub input: serde_json::Map<String, Value>,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub id: ToolId,
    pub invocation: ToolInvocation,
    pub outcome: Result<String, crate::tool_error::ToolFailure>,
}
