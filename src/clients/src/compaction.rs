use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "Vec<Value>", into = "Vec<Value>")]
pub struct CompactedWindow(Vec<Value>);

impl TryFrom<Vec<Value>> for CompactedWindow {
    type Error = anyhow::Error;

    fn try_from(items: Vec<Value>) -> anyhow::Result<Self> {
        let mut pending = std::collections::BTreeSet::new();
        for item in &items {
            match item["type"].as_str() {
                Some("compaction") => match item["encrypted_content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
                {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!("Incomplete encrypted compaction item")),
                }?,
                Some("function_call") => {
                    let id = item["call_id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| anyhow::anyhow!("Compacted call has no call_id"))?;
                    match pending.insert(id.to_owned()) {
                        true => Ok(()),
                        false => Err(anyhow::anyhow!("Duplicate call in compacted window")),
                    }?;
                }
                Some("function_call_output") => {
                    match item["call_id"]
                        .as_str()
                        .is_some_and(|id| pending.remove(id))
                    {
                        true => Ok(()),
                        false => Err(anyhow::anyhow!("Orphan tool result in compacted window")),
                    }?;
                }
                _ => {}
            }
        }
        let has_compaction = items.iter().any(|item| {
            item["type"] == "compaction"
                && item["encrypted_content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
        });
        match has_compaction
            && pending.is_empty()
            && items
                .iter()
                .all(|item| item.is_object() && item["type"].is_string())
        {
            true => Ok(Self(items)),
            false => Err(anyhow::anyhow!(
                "Compaction response lacks a complete encrypted compaction window"
            )),
        }
    }
}

impl From<CompactedWindow> for Vec<Value> {
    fn from(window: CompactedWindow) -> Self {
        window.0
    }
}

impl CompactedWindow {
    pub fn items(&self) -> &[Value] {
        &self.0
    }
}

#[derive(Debug, Deserialize)]
pub struct CompactionResponse {
    pub output: CompactedWindow,
    pub usage: crate::openai::Usage,
}

impl CompactionResponse {
    pub(crate) async fn from_stream(
        stream: impl Stream<Item = anyhow::Result<CompactionEvent>>,
    ) -> anyhow::Result<Self> {
        futures::pin_mut!(stream);
        let mut state = CompactionState::Pending;
        while let Some(event) = stream.next().await {
            state = state.advance(event?)?;
        }
        match state {
            CompactionState::Complete(response) => Ok(response),
            _ => Err(anyhow::anyhow!(
                "Compaction stream ended before response.completed"
            )),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum CompactionEvent {
    #[serde(rename = "response.output_item.done")]
    Item { item: Value },
    #[serde(rename = "response.completed")]
    Completed { response: CompactionStatus },
    #[serde(rename = "response.failed")]
    Failed { response: CompactionStatus },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: CompactionStatus },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: Option<String>,
        message: String,
    },
    #[serde(other)]
    Other,
}

impl CompactionEvent {
    pub(crate) fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Incomplete { .. }
                | Self::Error { .. }
        )
    }
}

#[derive(Deserialize)]
pub(crate) struct CompactionStatus {
    usage: Option<crate::openai::Usage>,
    error: Option<crate::openai::ResponseError>,
    incomplete_details: Option<crate::openai::IncompleteDetails>,
}

enum CompactionState {
    Pending,
    Collected(CompactedWindow),
    Complete(CompactionResponse),
}

impl CompactionState {
    fn advance(self, event: CompactionEvent) -> anyhow::Result<Self> {
        match (self, event) {
            (Self::Pending, CompactionEvent::Item { item }) if item["type"] == "compaction" => {
                CompactedWindow::try_from(vec![item]).map(Self::Collected)
            }
            (_, CompactionEvent::Item { item }) if item["type"] == "compaction" => Err(
                anyhow::anyhow!("Compaction stream returned more than one compaction item"),
            ),
            (Self::Collected(output), CompactionEvent::Completed { response }) => {
                Ok(Self::Complete(CompactionResponse {
                    output,
                    usage: response.usage.unwrap_or_default(),
                }))
            }
            (_, CompactionEvent::Completed { .. }) => Err(anyhow::anyhow!(
                "Compaction completed without an encrypted compaction item"
            )),
            (_, CompactionEvent::Failed { response }) => {
                let error = response.error.unwrap_or(crate::openai::ResponseError {
                    code: None,
                    message: "Compaction failed".into(),
                });
                Err(crate::failure::Failure::api(
                    error.code.as_deref().unwrap_or("failed_response"),
                    &error.message,
                )
                .into())
            }
            (_, CompactionEvent::Incomplete { response }) => {
                let reason = response
                    .incomplete_details
                    .map(|details| details.reason)
                    .unwrap_or_default();
                let code = match reason.as_str() {
                    "context_length_exceeded" | "context_window_exceeded" | "context_exceeded" => {
                        "context_length_exceeded"
                    }
                    "content_filter" => "content_filter",
                    _ => "incomplete_response",
                };
                Err(
                    crate::failure::Failure::api(code, &format!("Compaction incomplete: {reason}"))
                        .into(),
                )
            }
            (_, CompactionEvent::Error { code, message }) => Err(crate::failure::Failure::api(
                code.as_deref().unwrap_or("failed_response"),
                &message,
            )
            .into()),
            (state, _) => Ok(state),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct CompactionRequest {
    pub model: String,
    pub input: Vec<crate::openai::InputItem>,
    pub instructions: String,
}

#[cfg(test)]
#[path = "../tests/unit/compaction/tests.rs"]
mod tests;
