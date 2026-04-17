use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem, StreamAccu};
use crate::tool_call::ToolCall;
use anyhow::anyhow;
use clients::llm::{ContentBlockInfo, Delta};
use itertools::Itertools;
use std::collections::HashMap;
use tracing::error;

pub struct Batch {
    blocks: HashMap<usize, ContentBlock>,
}

impl Batch {
    pub fn new() -> Batch {
        Batch {
            blocks: Default::default(),
        }
    }

    pub fn has_tool(&self) -> bool {
        self.blocks.values().any(|x| {
            if let ContentBlock::Tool(_) = x {
                true
            } else {
                false
            }
        })
    }
    pub fn put(&mut self, index: usize, block: ContentBlock) {
        self.blocks.insert(index, block);
    }
    pub fn extract_and_pre_process(mut self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        let mut vec: Vec<(usize, ContentBlock)> = self.blocks.drain().into_iter().collect();
        vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
        let res: Result<_, _> = vec
            .into_iter()
            .map(|(i, item)| {
                let prep = item.process()?;
                Ok(PreprocessedStreamItem {
                    index: i,
                    processed: prep,
                })
            })
            .collect();
        res
    }

    pub fn accum(&mut self, index: &usize, delta: Delta) {
        match self.blocks.get_mut(index) {
            Some(block) => block.accum(delta),
            None => {
                error!("Content doesn't exist")
            }
        }
    }

    pub fn apply_reduce(&mut self, index: &usize, id: Option<String>) {
        match self.blocks.get_mut(index) {
            Some(block) => block.apply_reduce(id),
            None => {
                error!("Content doesn't exist")
            }
        }
    }

    pub fn get_delta_type(&self, index: &usize) -> Option<Delta> {
        match self.blocks.get(index) {
            Some(block) => block.get_delta_type(),
            None => {
                error!("Content doesn't exist");
                None
            }
        }
    }
}
// tracking current progress
pub enum ContentBlock {
    Tool(ContentData),
    Thinking(ContentData),
    Text(ContentData),
}

impl ContentBlock {
    pub fn new(index: usize, content_block_info: ContentBlockInfo) -> ContentBlock {
        match content_block_info {
            ContentBlockInfo::ToolUse { id, name, input: _ } => ContentBlock::Tool(ContentData {
                index,
                delta_buf: Default::default(),
                acc: vec![StreamAccu::Tool { id, name }],
            }),
            ContentBlockInfo::Thinking { thinking: _ } => ContentBlock::Thinking(ContentData {
                index,
                delta_buf: Default::default(),
                acc: Default::default(),
            }),
            ContentBlockInfo::Text { text: _ } => ContentBlock::Text(ContentData {
                index,
                delta_buf: Default::default(),
                acc: Default::default(),
            }),
        }
    }

    pub fn accum(&mut self, delta: Delta) {
        match self {
            ContentBlock::Tool(data) | ContentBlock::Thinking(data) | ContentBlock::Text(data) => {
                data.accum(delta)
            }
        }
    }

    pub fn get_delta_type(&self) -> Option<Delta> {
        match self {
            ContentBlock::Tool(data) | ContentBlock::Thinking(data) | ContentBlock::Text(data) => {
                data.get_delta_type()
            }
        }
    }

    pub fn apply_reduce(&mut self, id: Option<String>) {
        match self {
            ContentBlock::Tool(data) | ContentBlock::Thinking(data) | ContentBlock::Text(data) => {
                data.apply_reduce(id)
            }
        }
    }

    pub fn process(self) -> anyhow::Result<ProcessedItem> {
        match self {
            ContentBlock::Tool(tool) => {
                let toolcall = tool.extract_tool()?;
                Ok(ProcessedItem::Tool(toolcall))
            }
            ContentBlock::Thinking(thinking) => {
                if let Some(StreamAccu::Thinking {
                    thinking,
                    signature,
                    reasoning_id,
                }) = thinking.acc.first().cloned()
                {
                    Ok(ProcessedItem::Thinking {
                        thinking,
                        signature,
                        reasoning_id,
                    })
                } else {
                    Err(anyhow!("thinking can only be thinking"))
                }
            }
            ContentBlock::Text(text) => {
                if let Some(StreamAccu::String(text)) = text.acc.first().cloned() {
                    Ok(ProcessedItem::String(text))
                } else {
                    Err(anyhow!("thinking can only be thinking"))
                }
            }
        }
    }
}

pub struct ContentData {
    index: usize,
    delta_buf: Vec<Delta>,
    acc: Vec<StreamAccu>,
}

impl ContentData {
    pub fn accum(&mut self, delta: Delta) {
        self.delta_buf.push(delta);
    }

    pub fn get_delta_type(&self) -> Option<Delta> {
        self.delta_buf.first().cloned()
    }

    fn reduce(&mut self) -> Option<StreamAccu> {
        self.delta_buf
            .drain(..)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|delta| match delta {
                Delta::TextDelta { text } => Some(StreamAccu::String(text)),
                Delta::InputJsonDelta { partial_json } => Some(StreamAccu::Json(partial_json)),
                Delta::ThinkingDelta {
                    thinking,
                    reasoning_id,
                } => Some(StreamAccu::Thinking {
                    thinking,
                    signature: "".to_string(),
                    reasoning_id,
                }),
                Delta::SignatureDelta { signature } => Some(StreamAccu::Thinking {
                    thinking: "".to_string(),
                    signature,
                    reasoning_id: None,
                }),
            })
            .reduce(|mut acc, delta| {
                match (&mut acc, delta) {
                    (StreamAccu::String(buffer), StreamAccu::String(delta)) => {
                        buffer.push_str(&delta)
                    }
                    (
                        StreamAccu::Thinking {
                            thinking: think_buf,
                            signature: sig,
                            reasoning_id: acc_id,
                        },
                        StreamAccu::Thinking {
                            thinking,
                            signature,
                            reasoning_id,
                        },
                    ) => {
                        think_buf.push_str(&thinking);
                        if !signature.is_empty() {
                            sig.push_str(&signature)
                        }
                        if reasoning_id.is_some() {
                            *acc_id = reasoning_id;
                        }
                    }
                    (StreamAccu::Json(buffer), StreamAccu::Json(delta)) => buffer.push_str(&delta),
                    _ => {
                        unreachable!("mixed Text and InputJson deltas")
                    }
                }
                acc
            })
    }

    pub fn apply_reduce(&mut self, id: Option<String>) {
        match self.reduce() {
            Some(buf) => self.acc.push(buf.clone()),
            // special case here for openai
            None => {
                id.map(|t| {
                    if t.starts_with("rs_") && self.acc.is_empty() {
                        self.acc.push(StreamAccu::Thinking {
                            thinking: "".to_string(),
                            signature: "".to_string(),
                            reasoning_id: Some(t),
                        });
                    } else {
                        ()
                    }
                });
            }
        }
    }

    pub fn extract_tool(self) -> anyhow::Result<ToolCall> {
        let tool_info = self
            .acc
            .get(0)
            .cloned()
            .map(|t| match t {
                StreamAccu::Tool { id, name } => Ok(StreamAccu::Tool { id, name }),
                _ => Err(anyhow::Error::msg("Tool doesn't exist")),
            })
            .transpose()?;
        let json = self
            .acc
            .get(1)
            .cloned()
            .map(|j| match j {
                StreamAccu::Json(json) => Ok(json),
                _ => Err(anyhow::Error::msg("Json doesn't exist")),
            })
            .transpose()?;

        if let Some(StreamAccu::Tool { id, name }) = tool_info
            && let Some(json) = json
        {
            Ok(ToolCall { id, name, json })
        } else {
            Err(anyhow::Error::msg("Type shit"))
        }
    }
}
