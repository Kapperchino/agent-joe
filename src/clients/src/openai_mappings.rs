use crate::{llm, openai, tool_defs};
use crate::llm::ContentBlock;
use crate::openai::{ClientRequest, InputItem, Role};
use crate::tool_defs::Tool;

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
            tools: vec![],
        };
        todo!()
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

impl From<tool_defs::Tool> for openai::Tool{
    fn from(value: Tool) -> Self {
        todo!()
    }
}
