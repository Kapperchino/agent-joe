use crate::{
    actor::ActorContext,
    session::artifacts::{ArtifactPage, ArtifactRange},
};
use analysis::contexts::context::Context;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tools::tool_defs::{ToolDefTrait, ToolEffect, ToolId, ToolTrait, ToolType};
use turbo_code_macros::{ToolDef, ToolInput};
use utils::utils::FnvHashMap;

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolDef)]
#[tool(
    name = "read_artifact",
    description = "Read a saved full tool output in this conversation. Byte offsets must be UTF-8 boundaries. Follow next_offset to retrieve the next page."
)]
pub struct ReadArtifact {
    #[tool(input)]
    pub input: ReadArtifactInput,
}

impl std::fmt::Display for ReadArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Read artifact {} at byte {}",
            self.input.id, self.input.offset
        )
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone, ToolInput)]
pub struct ReadArtifactInput {
    #[tool(description = "Artifact ID from a tool result", required)]
    pub id: String,
    #[tool(description = "Zero-based byte offset, initially 0", required)]
    pub offset: usize,
    #[tool(description = "Page size in bytes, 1 through 4096", required)]
    pub bytes: usize,
}

#[async_trait]
impl<C: Context> ToolTrait<C, ActorContext<C>> for ReadArtifact {
    type Input = ReadArtifactInput;
    type Output = ArtifactPage;

    async fn run(
        input: Self::Input,
        _: ToolId,
        _: &C,
        actor: &ActorContext<C>,
    ) -> anyhow::Result<Self::Output> {
        let session = match actor {
            ActorContext::ActorInfo(info) => info.dep.runtime.session.as_ref(),
            ActorContext::Noop => None,
        }
        .ok_or_else(|| anyhow::anyhow!("Session storage is unavailable"))?;
        session.read_artifact(&input.id, ArtifactRange::new(input.offset, input.bytes)?)
    }

    fn display_input(input: &Self::Input) -> String {
        format!("Read artifact {} at byte {}", input.id, input.offset)
    }

    fn req_from_input(input: &Self::Input) -> anyhow::Result<FnvHashMap<String, String>> {
        ReadArtifact {
            input: input.clone(),
        }
        .req()
    }

    fn output_to_content(_: &Self::Input, output: &Self::Output) -> anyhow::Result<String> {
        Ok(format!(
            "Artifact {} ({} bytes), offset {}, next_offset: {:?}\n{}",
            output.artifact.id,
            output.artifact.bytes,
            output.offset,
            output.next_offset,
            output.content
        ))
    }

    fn effect() -> ToolEffect {
        ToolEffect::Read
    }

    fn tool_type() -> ToolType {
        ToolType::Client
    }
}
