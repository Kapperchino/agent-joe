use crate::stream_processor::{PreprocessedStreamItem, ProcessedItem, StreamAccu};
use crate::tool_call::ToolCall;
use anyhow::anyhow;
use clients::llm::{ContentBlockInfo, Delta};
use std::collections::HashMap;

// tracking current progress
pub enum Batch {
    Tool(BatchData),
    Thinking(BatchData),
    Text(BatchData),
}

impl Batch {
    pub fn new(index: usize, content_block_info: ContentBlockInfo) -> Batch {
        match content_block_info {
            ContentBlockInfo::ToolUse { id, name, input: _ } => Batch::Tool(BatchData {
                delta_buf: Default::default(),
                acc_map: HashMap::from([(index, vec![StreamAccu::Tool { id, name }])]),
            }),
            ContentBlockInfo::Thinking { thinking: _ } => Batch::Thinking(BatchData {
                delta_buf: Default::default(),
                acc_map: Default::default(),
            }),
            ContentBlockInfo::Text { text: _ } => Batch::Text(BatchData {
                delta_buf: Default::default(),
                acc_map: Default::default(),
            }),
        }
    }

    pub fn accum(&mut self, index: usize, delta: Delta) {
        match self {
            Batch::Tool(data) | Batch::Thinking(data) | Batch::Text(data) => {
                data.accum(index, delta)
            }
        }
    }

    pub fn get_delta_type(&self, index: &usize) -> Option<Delta> {
        match self {
            Batch::Tool(data) | Batch::Thinking(data) | Batch::Text(data) => {
                data.get_delta_type(index)
            }
        }
    }

    pub fn apply_reduce(&mut self, index: usize, id: Option<String>) {
        match self {
            Batch::Tool(data) | Batch::Thinking(data) | Batch::Text(data) => {
                data.apply_reduce(index, id)
            }
        }
    }

    pub fn extract_and_pre_process(self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        match self {
            Batch::Tool(data) | Batch::Thinking(data) | Batch::Text(data) => {
                data.extract_and_pre_process()
            }
        }
    }
}

pub struct BatchData {
    delta_buf: HashMap<usize, Vec<Delta>>,
    acc_map: HashMap<usize, Vec<StreamAccu>>,
}

impl BatchData {
    pub fn accum(&mut self, index: usize, delta: Delta) {
        match self.delta_buf.get_mut(&index) {
            None => {
                self.delta_buf.insert(index, vec![delta]);
            }
            Some(vec) => vec.push(delta),
        }
    }

    pub fn get_delta_type(&self, index: &usize) -> Option<Delta> {
        self.delta_buf
            .get(&index)
            .and_then(|vec| vec.first())
            .cloned()
    }

    fn reduce(&mut self, index: usize) -> Option<StreamAccu> {
        self.delta_buf.remove(&index).and_then(|buf| {
            buf.into_iter()
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
                        (StreamAccu::Json(buffer), StreamAccu::Json(delta)) => {
                            buffer.push_str(&delta)
                        }
                        _ => {
                            unreachable!("mixed Text and InputJson deltas")
                        }
                    }
                    acc
                })
        })
    }

    pub fn apply_reduce(&mut self, index: usize, id: Option<String>) {
        match self.reduce(index) {
            Some(buf) => match self.acc_map.get_mut(&index) {
                Some(vec) => vec.push(buf.clone()),
                None => {
                    self.acc_map.insert(index, vec![buf.clone()]);
                }
            },
            // special case here for openai
            None => {
                id.map(|t| {
                    if t.starts_with("rs_") && !self.acc_map.contains_key(&index) {
                        self.acc_map.insert(
                            index,
                            vec![StreamAccu::Thinking {
                                thinking: "".to_string(),
                                signature: "".to_string(),
                                reasoning_id: Some(t),
                            }],
                        );
                    } else {
                        ()
                    }
                });
            }
        }
    }

    pub fn extract_and_pre_process(mut self) -> anyhow::Result<Vec<PreprocessedStreamItem>> {
        let mut vec: Vec<(usize, Vec<StreamAccu>)> = self.acc_map.drain().into_iter().collect();
        vec.sort_by(|(i1, _), (i2, _)| i1.cmp(i2));
        let res: Result<_, _> = vec
            .into_iter()
            .map(|(i, item)| {
                let prep = if let Some(StreamAccu::Tool { .. }) = item.first() {
                    let toolcall = Self::extract_tool((i, item))?;
                    Ok(ProcessedItem::Tool(toolcall))
                } else {
                    match item.first().cloned() {
                        Some(accu) => Ok(match accu {
                            StreamAccu::String(s) => ProcessedItem::String(s),
                            StreamAccu::Thinking {
                                thinking,
                                signature,
                                reasoning_id,
                            } => ProcessedItem::Thinking {
                                thinking,
                                signature,
                                reasoning_id,
                            },
                            _ => unreachable!("Should not be here"),
                        }),
                        None => Err(anyhow!("Empty stream process")),
                    }
                }?;
                Ok(PreprocessedStreamItem {
                    index: i,
                    processed: prep,
                })
            })
            .collect();
        res
    }

    fn extract_tool((_, vec): (usize, Vec<StreamAccu>)) -> anyhow::Result<ToolCall> {
        let tool_info = vec
            .get(0)
            .cloned()
            .map(|t| match t {
                StreamAccu::Tool { id, name } => Ok(StreamAccu::Tool { id, name }),
                _ => Err(anyhow::Error::msg("Tool doesn't exist")),
            })
            .transpose()?;
        let json = vec
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
