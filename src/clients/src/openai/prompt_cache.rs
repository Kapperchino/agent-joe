use super::{InputItem, OpenAIConfig, ReasoningConfig, ResponseRequest, Role, Tool};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputContent {
    Text(String),
    Blocks(Vec<InputText>),
}

impl From<String> for InputContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename = "input_text")]
pub struct InputText {
    text: String,
    prompt_cache_breakpoint: CacheBreakpoint,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum CacheBreakpoint {
    Explicit,
}

#[derive(Debug, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum PromptCacheOptions {
    Implicit,
}

impl PromptCacheOptions {
    pub(super) fn for_model(config: &OpenAIConfig, model: &str) -> Option<Self> {
        let supported = [
            "gpt-6-astra",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
        ]
        .iter()
        .any(|name| {
            model == *name
                || model
                    .strip_prefix(name)
                    .is_some_and(|suffix| suffix.starts_with("-20"))
        });
        (config.supports_prompt_cache_options() && supported).then_some(Self::Implicit)
    }
}

impl InputContent {
    fn with_cache_breakpoint(self) -> Self {
        match self {
            Self::Text(text) => Self::Blocks(vec![InputText {
                text,
                prompt_cache_breakpoint: CacheBreakpoint::Explicit,
            }]),
            content => content,
        }
    }
}

impl InputItem {
    pub(super) fn with_cache_breakpoint(self) -> Self {
        match self {
            Self::Message {
                role: Role::User,
                content,
                phase,
            } => Self::Message {
                role: Role::User,
                content: content.with_cache_breakpoint(),
                phase,
            },
            Self::FunctionCallOutput { call_id, output } => Self::FunctionCallOutput {
                call_id,
                output: output.with_cache_breakpoint(),
            },
            item => item,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RequestFingerprint {
    pub(super) settings: String,
    pub(super) prefixes: Vec<String>,
}

#[derive(Serialize)]
struct CacheSettings<'a> {
    model: &'a str,
    instructions: &'a str,
    tools: &'a [Tool],
    reasoning: &'a Option<ReasoningConfig>,
    parallel_tool_calls: bool,
    prompt_cache_key: &'a Option<String>,
    prompt_cache_options: &'a Option<PromptCacheOptions>,
}

impl RequestFingerprint {
    pub(super) fn new(request: &ResponseRequest) -> serde_json::Result<Self> {
        let settings = CacheSettings {
            model: &request.model,
            instructions: &request.instructions,
            tools: &request.tools,
            reasoning: &request.reasoning,
            parallel_tool_calls: request.parallel_tool_calls,
            prompt_cache_key: &request.prompt_cache_key,
            prompt_cache_options: &request.prompt_cache_options,
        };
        let hash = Sha256::new_with_prefix(serde_json::to_vec(&settings)?);
        let settings = Self::hex(&hash);
        let prefixes = request
            .input
            .iter()
            .scan(hash, |hash, item| {
                Some(serde_json::to_vec(item).map(|bytes| {
                    hash.update(bytes);
                    Self::hex(hash)
                }))
            })
            .collect::<serde_json::Result<Vec<_>>>()?;
        Ok(Self { settings, prefixes })
    }

    fn hex(hash: &Sha256) -> String {
        hash.clone()
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

impl ResponseRequest {
    pub(super) fn log_cache_fingerprint(&self) {
        match tracing::enabled!(tracing::Level::DEBUG)
            .then(|| RequestFingerprint::new(self))
            .transpose()
        {
            Ok(Some(fingerprint)) => tracing::debug!(
                model = self.model,
                prompt_cache_key = self.prompt_cache_key,
                cache_settings_hash = fingerprint.settings,
                cache_prefix_hashes = ?fingerprint.prefixes,
                input_items = self.input.len(),
                "Provider request cache fingerprint"
            ),
            Err(error) => tracing::debug!(%error, "Cannot fingerprint provider request"),
            Ok(None) => (),
        }
    }
}
