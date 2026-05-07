use serde_json::Value;
use tools::tool_defs::ToolId;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: ToolId,
    pub name: String,
    pub json: String,
}

impl ToolCall {
    pub fn input_value(&self) -> anyhow::Result<Value> {
        if self.json.trim().is_empty() {
            Ok(Value::Object(Default::default()))
        } else {
            Ok(serde_json::from_str(&self.json)?)
        }
    }
}
