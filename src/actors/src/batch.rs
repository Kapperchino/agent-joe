use crate::stream_processor::ProcessedItem;
use crate::tool_call::ToolCall;
use anyhow::anyhow;
use clients::llm::PendingToolId;
use clients::llm::{ContentBlock as MessageContent, ContentBlockInfo, Delta};
use std::collections::BTreeMap;
use tools::tool_defs::NonEmptyString;

#[derive(Default)]
pub struct Batch {
    blocks: BTreeMap<usize, ContentBlock>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn completed_content(&self) -> Vec<MessageContent> {
        self.blocks
            .values()
            .filter_map(|block| match block {
                ContentBlock::Complete(content)
                    if !matches!(
                        content,
                        MessageContent::ToolBlock { .. } | MessageContent::ToolResult { .. }
                    ) =>
                {
                    Some(content.clone())
                }
                _ => None,
            })
            .collect()
    }

    pub fn has_tool(&self) -> bool {
        self.blocks
            .values()
            .any(|block| matches!(block.kind(), ContentKind::Tool))
    }

    pub fn put(&mut self, index: usize, block: ContentBlock) {
        self.blocks.insert(index, block);
    }

    pub fn complete_item(&mut self, index: usize, content: MessageContent) {
        self.blocks.insert(index, ContentBlock::Complete(content));
    }

    pub fn extract_and_pre_process(&mut self) -> anyhow::Result<Vec<ProcessedItem>> {
        let result = self
            .blocks
            .iter()
            .map(|(&index, block)| match block {
                ContentBlock::Pending(_) => Err(anyhow!("Content block {index} is incomplete")),
                ContentBlock::Complete(content) => {
                    let processed = match content {
                        MessageContent::ToolBlock {
                            tool_id,
                            name,
                            input,
                        } => ProcessedItem::Tool(ToolCall {
                            id: tool_id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        }),
                        content => ProcessedItem::Content(content.clone()),
                    };
                    Ok(processed)
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.blocks.clear();
        Ok(result)
    }

    pub fn accum(&mut self, index: &usize, delta: Delta) -> anyhow::Result<()> {
        let block = self
            .blocks
            .get_mut(index)
            .ok_or_else(|| anyhow!("Missing content block {index}"))?;
        *block = block.accum(delta)?;
        Ok(())
    }

    pub fn apply_reduce(
        &mut self,
        index: &usize,
        id: Option<NonEmptyString>,
    ) -> anyhow::Result<()> {
        let block = self
            .blocks
            .get_mut(index)
            .ok_or_else(|| anyhow!("Missing content block {index}"))?;
        if let ContentBlock::Pending(content) = block {
            *block = ContentBlock::Complete(content.complete(id)?);
        }
        Ok(())
    }

    pub fn content_kind(&self, index: &usize) -> Option<ContentKind> {
        self.blocks.get(index).map(ContentBlock::kind)
    }
}

pub enum ContentKind {
    Text,
    Thinking,
    Tool,
}

pub enum ContentBlock {
    Pending(PendingContent),
    Complete(MessageContent),
}

pub enum PendingContent {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
        reasoning_id: Option<String>,
    },
    Tool {
        id: PendingToolId,
        name: NonEmptyString,
        input: serde_json::Map<String, serde_json::Value>,
        partial_json: String,
    },
}

impl ContentBlock {
    pub fn new(info: ContentBlockInfo) -> Self {
        Self::Pending(match info {
            ContentBlockInfo::ToolUse { id, name, input } => PendingContent::Tool {
                id,
                name,
                input,
                partial_json: String::new(),
            },
            ContentBlockInfo::Thinking { thinking } => PendingContent::Thinking {
                thinking,
                signature: String::new(),
                reasoning_id: None,
            },
            ContentBlockInfo::Text { text } => PendingContent::Text(text),
        })
    }

    fn kind(&self) -> ContentKind {
        match self {
            Self::Pending(PendingContent::Tool { .. })
            | Self::Complete(MessageContent::ToolBlock { .. }) => ContentKind::Tool,
            Self::Pending(PendingContent::Thinking { .. })
            | Self::Complete(
                MessageContent::ThinkingBlock { .. } | MessageContent::OpenAIReasoning(_),
            ) => ContentKind::Thinking,
            Self::Pending(PendingContent::Text(_))
            | Self::Complete(
                MessageContent::MessageBlock { .. } | MessageContent::ToolResult { .. },
            ) => ContentKind::Text,
        }
    }

    fn accum(&self, delta: Delta) -> anyhow::Result<Self> {
        match self {
            Self::Complete(_) => Err(anyhow!("Received a delta after content block completion")),
            Self::Pending(content) => content.accum(delta).map(Self::Pending),
        }
    }
}

impl PendingContent {
    fn accum(&self, delta: Delta) -> anyhow::Result<Self> {
        match (self, delta) {
            (Self::Text(text), Delta::TextDelta { text: delta }) => {
                Ok(Self::Text(format!("{text}{delta}")))
            }
            (
                Self::Tool {
                    id,
                    name,
                    input,
                    partial_json,
                },
                Delta::InputJsonDelta {
                    partial_json: delta,
                },
            ) => Ok(Self::Tool {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                partial_json: format!("{partial_json}{delta}"),
            }),
            (
                Self::Thinking {
                    thinking,
                    signature,
                    reasoning_id,
                },
                Delta::ThinkingDelta {
                    thinking: delta,
                    reasoning_id: id,
                },
            ) => Ok(Self::Thinking {
                thinking: format!("{thinking}{delta}"),
                signature: signature.clone(),
                reasoning_id: id.or_else(|| reasoning_id.clone()),
            }),
            (
                Self::Thinking {
                    thinking,
                    signature,
                    reasoning_id,
                },
                Delta::SignatureDelta { signature: delta },
            ) => Ok(Self::Thinking {
                thinking: thinking.clone(),
                signature: format!("{signature}{delta}"),
                reasoning_id: reasoning_id.clone(),
            }),
            _ => Err(anyhow!("Delta does not match its content block")),
        }
    }

    fn complete(&self, completed_id: Option<NonEmptyString>) -> anyhow::Result<MessageContent> {
        match self {
            Self::Text(text) => Ok(MessageContent::MessageBlock {
                text: text.clone(),
                phase: None,
            }),
            Self::Thinking {
                thinking,
                signature,
                reasoning_id,
            } => Ok(MessageContent::ThinkingBlock {
                thinking: thinking.clone(),
                signature: signature.clone(),
                reasoning_id: reasoning_id.clone(),
            }),
            Self::Tool {
                id,
                name,
                input,
                partial_json,
            } => {
                let input = if partial_json.is_empty() {
                    input.clone()
                } else {
                    serde_json::from_str(partial_json)?
                };
                let tool_id = id.complete(completed_id)?;
                Ok(MessageContent::ToolBlock {
                    tool_id,
                    name: name.clone(),
                    input,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_delta_preserves_pending_content_for_subsequent_updates() {
        let mut batch = Batch::new();
        batch.put(
            0,
            ContentBlock::new(ContentBlockInfo::Text {
                text: "First".into(),
            }),
        );

        assert!(
            batch
                .accum(
                    &0,
                    Delta::InputJsonDelta {
                        partial_json: "{}".into(),
                    },
                )
                .is_err()
        );
        batch
            .accum(
                &0,
                Delta::TextDelta {
                    text: " second".into(),
                },
            )
            .unwrap();
        batch.apply_reduce(&0, None).unwrap();

        let items = batch.extract_and_pre_process().unwrap();
        assert!(matches!(
            &items[0],
            ProcessedItem::Content(MessageContent::MessageBlock { text, .. })
                if text == "First second"
        ));
    }

    #[test]
    fn rejected_delta_preserves_completed_content() {
        let mut batch = Batch::new();
        batch.complete_item(
            0,
            MessageContent::MessageBlock {
                text: "Final".into(),
                phase: None,
            },
        );

        assert!(
            batch
                .accum(
                    &0,
                    Delta::TextDelta {
                        text: " discarded".into(),
                    },
                )
                .is_err()
        );

        let items = batch.extract_and_pre_process().unwrap();
        assert!(matches!(
            &items[0],
            ProcessedItem::Content(MessageContent::MessageBlock { text, .. })
                if text == "Final"
        ));
    }
}
