use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem};
use crate::tool_call::ToolCall;
use anyhow::{anyhow, bail, ensure};
use clients::llm::{ContentBlock as MessageContent, ContentBlockInfo, Delta};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Batch {
    blocks: BTreeMap<usize, ContentBlock>,
}

impl Batch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_tool(&self) -> bool {
        self.blocks
            .values()
            .any(|block| matches!(block.content, MessageContent::ToolBlock { .. }))
    }

    pub fn put(&mut self, index: usize, block: ContentBlock) {
        self.blocks.insert(index, block);
    }

    pub fn complete_item(&mut self, index: usize, content: MessageContent) {
        // output_item.done is authoritative, including opaque reasoning and phase.
        self.blocks.insert(
            index,
            ContentBlock {
                content,
                partial_json: String::new(),
                complete: true,
            },
        );
    }

    pub fn extract_and_pre_process(&mut self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        // Validate the entire response before dispatching any side effects.
        let result = self
            .blocks
            .iter()
            .map(|(&index, block)| {
                ensure!(block.complete, "Content block {index} is incomplete");
                let processed = match &block.content {
                    MessageContent::ToolBlock {
                        tool_id,
                        name,
                        input,
                    } => {
                        ensure!(
                            !name.is_empty() && !tool_id.id.is_empty(),
                            "Tool name and ID must be present"
                        );
                        ensure!(
                            tool_id.call_id.as_ref().is_none_or(|id| !id.is_empty()),
                            "Tool call_id must not be empty"
                        );
                        ensure!(
                            input.is_object(),
                            "Tool {name} arguments must be a JSON object"
                        );
                        ProcessedItem::Tool(ToolCall {
                            id: tool_id.clone(),
                            name: name.clone(),
                            json: serde_json::to_string(input)?,
                        })
                    }
                    content => ProcessedItem::Content(content.clone()),
                };
                Ok(PreprocessedStreamItem { index, processed })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.blocks.clear();
        Ok(result)
    }

    pub fn accum(&mut self, index: &usize, delta: Delta) -> anyhow::Result<()> {
        self.blocks
            .get_mut(index)
            .ok_or_else(|| anyhow!("Missing content block {index}"))?
            .accum(delta)
    }

    pub fn apply_reduce(&mut self, index: &usize, id: Option<String>) -> anyhow::Result<()> {
        let block = self
            .blocks
            .get_mut(index)
            .ok_or_else(|| anyhow!("Missing content block {index}"))?;
        if let MessageContent::ToolBlock { tool_id, input, .. } = &mut block.content {
            if !block.partial_json.is_empty() {
                *input = serde_json::from_str(&block.partial_json)?;
            }
            if let Some(id) = id {
                tool_id.id = id;
            }
        }
        block.complete = true;
        Ok(())
    }

    pub fn get_delta_type(&self, index: &usize) -> Option<Delta> {
        self.blocks.get(index).map(|block| match &block.content {
            MessageContent::ToolBlock { .. } => Delta::InputJsonDelta {
                partial_json: String::new(),
            },
            MessageContent::ThinkingBlock { .. } | MessageContent::OpenAIReasoning(_) => {
                Delta::ThinkingDelta {
                    thinking: String::new(),
                    reasoning_id: None,
                }
            }
            _ => Delta::TextDelta {
                text: String::new(),
            },
        })
    }
}

pub struct ContentBlock {
    content: MessageContent,
    partial_json: String,
    complete: bool,
}

impl ContentBlock {
    pub fn new(_index: usize, info: ContentBlockInfo) -> Self {
        let content = match info {
            ContentBlockInfo::ToolUse { id, name, input } => MessageContent::ToolBlock {
                tool_id: id,
                name,
                input,
            },
            ContentBlockInfo::Thinking { thinking } => MessageContent::ThinkingBlock {
                thinking,
                signature: String::new(),
                reasoning_id: None,
            },
            ContentBlockInfo::Text { text } => MessageContent::MessageBlock { text, phase: None },
        };
        Self {
            content,
            partial_json: String::new(),
            complete: false,
        }
    }

    fn accum(&mut self, delta: Delta) -> anyhow::Result<()> {
        ensure!(
            !self.complete,
            "Received a delta after content block completion"
        );
        match (&mut self.content, delta) {
            (MessageContent::MessageBlock { text, .. }, Delta::TextDelta { text: delta }) => {
                text.push_str(&delta)
            }
            (MessageContent::ToolBlock { .. }, Delta::InputJsonDelta { partial_json }) => {
                self.partial_json.push_str(&partial_json)
            }
            (
                MessageContent::ThinkingBlock {
                    thinking,
                    reasoning_id,
                    ..
                },
                Delta::ThinkingDelta {
                    thinking: delta,
                    reasoning_id: id,
                },
            ) => {
                thinking.push_str(&delta);
                if id.is_some() {
                    *reasoning_id = id;
                }
            }
            (
                MessageContent::ThinkingBlock { signature, .. },
                Delta::SignatureDelta { signature: delta },
            ) => signature.push_str(&delta),
            _ => bail!("Delta does not match its content block"),
        }
        Ok(())
    }
}
