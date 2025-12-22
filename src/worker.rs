use crate::actor::StreamAccu;
use crate::tools;
use crate::tools::{ListFilesInput, ReadFileInput, ToolResult};
use anyhow::Error;
use futures::future;

// Base unit for the agent, should be given context and then simply do the work
pub struct Worker {}
impl Worker {
    async fn tool_use(
        a_vec: &Vec<StreamAccu>,
        name: String,
        id: String,
    ) -> Result<ToolResult, anyhow::Error> {
        match tools::Tool::from_str(name.as_str())? {
            tools::Tool::ReadFile(_) => {
                match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
                    StreamAccu::Json(json) => {
                        let input: ReadFileInput = serde_json::from_str::<_>(json)?;
                        let rf = tools::ReadFile {
                            id: id.clone(),
                            input,
                        };
                        Ok(tools::Tool::ReadFile(rf).use_tool(id).await?)
                    }
                    _ => Err(anyhow::Error::msg("doesn't work")),
                }
            }
            tools::Tool::ListFiles(_) => {
                match a_vec.get(1).ok_or(anyhow::Error::msg("doesn't work"))? {
                    StreamAccu::Json(json) => {
                        let input: ListFilesInput = serde_json::from_str::<_>(json)?;
                        let rf = tools::ListFiles {
                            id: id.clone(),
                            input,
                        };
                        Ok(tools::Tool::ListFiles(rf).use_tool(id).await?)
                    }
                    _ => Err(anyhow::Error::msg("doesn't work")),
                }
            }
        }
    }

    pub async fn process_tools(
        vec: Vec<(usize, Vec<StreamAccu>)>,
    ) -> Vec<Result<crate::actor::StreamRes, Error>> {
        let futures: Vec<_> = vec
            .into_iter()
            .map(
                async |(_, a_vec): (usize, Vec<StreamAccu>)| match a_vec.first() {
                    Some(accu) => match accu {
                        StreamAccu::String(str) => Ok(crate::actor::StreamRes::String(str.clone())),
                        StreamAccu::Tool { id, name } => {
                            let tool_res =
                                Worker::tool_use(&a_vec, name.to_string(), id.to_string()).await?;
                            Ok(crate::actor::StreamRes::Tool(tool_res))
                        }
                        StreamAccu::Thinking {
                            thinking,
                            signature,
                        } => Ok(crate::actor::StreamRes::Thinking {
                            thinking: thinking.clone(),
                            signature: signature.clone(),
                        }),
                        _ => Err(anyhow::Error::msg("No valid tool")),
                    },
                    None => Err(anyhow::Error::msg("No valid tool")),
                },
            )
            .collect();

        future::join_all(futures).await
    }
}
