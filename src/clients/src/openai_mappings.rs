use crate::llm::ContentBlock;
use crate::openai::{ClientRequest, InputItem, Role};
use crate::{llm, openai, tool_defs};

impl From<llm::ClientRequest> for ClientRequest {
    fn from(llm_req: llm::ClientRequest) -> Self {
        ClientRequest {
            input: llm_req
                .messages
                .into_iter()
                .flat_map(|x| {
                    let role = x.role.clone();
                    x.content.into_iter().map(move |c| (c, role.clone()))
                })
                .map(|(content, role)| {
                    match content {
                        ContentBlock::MessageBlock { text } => InputItem::Message {
                            role: role.into(),
                            content: text,
                        },
                        ContentBlock::ThinkingBlock { thinking, .. } => InputItem::Message {
                            role: role.into(),
                            content: thinking,
                        },
                        ContentBlock::ToolBlock { id, name, input } => {
                            // doesn't matter here
                            InputItem::Message {
                                role: role.into(),
                                content: name,
                            }
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => InputItem::FunctionCallOutput {
                            call_id: tool_use_id,
                            output: content,
                        },
                    }
                })
                .collect(),
            instructions: llm_req.system,
            model: llm_req.model,
            tools: llm_req.tools.into_iter().map(|t| t.into()).collect(),
        }
    }
}

impl From<llm::Role> for Role {
    fn from(value: llm::Role) -> Self {
        match value {
            llm::Role::User => Role::User,
            llm::Role::Assistant => Role::Assistant,
        }
    }
}

impl From<tool_defs::Tool> for openai::Tool {
    fn from(value: tool_defs::Tool) -> Self {
        use tool_defs::ToolDefTrait;
        macro_rules! extract {
            ($variant:ident) => {{
                (
                    tool_defs::$variant::tool_name(),
                    tool_defs::$variant::tool_description(),
                    tool_defs::$variant::field_properties(),
                    tool_defs::$variant::required_fields(),
                )
            }};
        }

        let (name, description, properties, required) = match value {
            tool_defs::Tool::ReadFile(_) => extract!(ReadFile),
            tool_defs::Tool::InsertAfterLine(_) => extract!(InsertAfterLine),
            tool_defs::Tool::StringReplace(_) => extract!(StringReplace),
            tool_defs::Tool::CargoCheck(_) => extract!(CargoCheck),
        };

        openai::Tool {
            tool_type: "function".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            parameters: openai::FunctionParameters {
                param_type: "object".to_string(),
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
                required,
            },
        }
    }
}

impl From<tool_defs::ToolProperty> for openai::ToolProperty {
    fn from(value: tool_defs::ToolProperty) -> Self {
        match value {
            tool_defs::ToolProperty::Value {
                name,
                prop_type,
                description,
            } => openai::ToolProperty::Value {
                name,
                prop_type,
                description,
            },
            tool_defs::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties,
            } => openai::ToolProperty::Object {
                name,
                prop_type,
                description,
                properties: properties.into_iter().map(|(k, v)| (k, v.into())).collect(),
            },
        }
    }
}
