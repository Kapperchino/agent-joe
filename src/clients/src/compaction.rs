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
mod tests {
    use super::*;
    use crate::{
        LocalOpenAIConfig, OpenAIAuthConfig, OpenAIConfig, OpenAIEffort,
        llm::{ClientRequest, Message},
        openai::OpenAIClient,
    };
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

    async fn streamed(events: Vec<Value>) -> anyhow::Result<CompactionResponse> {
        let data = events
            .iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect::<String>();
        let bytes = futures::stream::iter(
            data.into_bytes()
                .into_iter()
                .map(|byte| Ok::<_, std::io::Error>(vec![byte])),
        );
        CompactionResponse::from_stream(crate::sse::decode(bytes, CompactionEvent::terminal)).await
    }

    fn compaction_item() -> Value {
        json!({"type":"compaction", "id":"cmp-1", "encrypted_content":"opaque", "future":{"preserve":"終😀"}})
    }

    fn item_done(item: Value) -> Value {
        json!({"type":"response.output_item.done", "item":item, "output_index":0})
    }

    fn completed() -> Value {
        json!({"type":"response.completed", "response":{"output":[compaction_item()], "usage":{"input_tokens":1234,"output_tokens":90}}})
    }

    #[tokio::test]
    async fn streaming_compaction_preserves_opaque_state_and_completion_usage() {
        let response = streamed(vec![
            json!({"type":"response.created", "response":{"id":"resp-1"}}),
            json!({"type":"response.output_item.added", "item":{"type":"compaction"}}),
            json!({"type":"response.keepalive"}),
            item_done(compaction_item()),
            completed(),
        ])
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(response.output).unwrap(),
            json!([compaction_item()])
        );
        assert_eq!(response.usage.input_tokens, 1234);
        assert_eq!(response.usage.output_tokens, 90);
    }

    #[tokio::test]
    async fn streaming_compaction_rejects_partial_missing_duplicate_and_invalid_state() {
        for events in [
            vec![item_done(compaction_item())],
            vec![completed()],
            vec![
                item_done(compaction_item()),
                item_done(compaction_item()),
                completed(),
            ],
            vec![
                item_done(json!({"type":"compaction", "encrypted_content":""})),
                completed(),
            ],
            vec![
                item_done(json!({"type":"message", "role":"assistant", "content":"summary"})),
                completed(),
            ],
        ] {
            assert!(streamed(events).await.is_err());
        }
    }

    #[tokio::test]
    async fn streaming_compaction_propagates_terminal_failures_after_an_output_item() {
        use crate::failure::{Failure, FailureKind};
        struct FailureCase {
            event: Value,
            kind: FailureKind,
        }
        for case in [
            FailureCase {
                event: json!({"type":"error", "code":"rate_limit_exceeded", "message":"slow down"}),
                kind: FailureKind::RateLimit,
            },
            FailureCase {
                event: json!({"type":"response.failed", "response":{"error":{"code":"server_error", "message":"failed"}}}),
                kind: FailureKind::Transport,
            },
            FailureCase {
                event: json!({"type":"response.incomplete", "response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
                kind: FailureKind::Truncation,
            },
            FailureCase {
                event: json!({"type":"response.incomplete", "response":{"incomplete_details":{"reason":"context_length_exceeded"}}}),
                kind: FailureKind::ContextOverflow,
            },
        ] {
            let error = streamed(vec![item_done(compaction_item()), case.event])
                .await
                .unwrap_err();
            assert_eq!(error.downcast_ref::<Failure>().unwrap().kind, case.kind);
        }
    }

    #[test]
    fn malformed_native_windows_and_orphan_exchanges_are_rejected() {
        let compact = json!({"type": "compaction", "encrypted_content": "opaque"});
        for items in [
            json!([]),
            json!([{"type": "compaction"}]),
            json!([compact.clone(), {"type": "function_call", "call_id": "orphan"}]),
            json!([compact.clone(), {"type": "function_call_output", "call_id": "orphan"}]),
            json!([compact.clone(), {"type": "compaction", "encrypted_content": ""}]),
        ] {
            assert!(serde_json::from_value::<CompactedWindow>(items).is_err());
        }
        assert!(serde_json::from_value::<CompactedWindow>(json!([compact, {"type": "function_call", "call_id": "paired"}, {"type": "function_call_output", "call_id": "paired", "output": "done"}])).is_ok());
    }

    struct CapturedRequest {
        path: String,
        body: Value,
    }

    #[tokio::test]
    async fn native_endpoint_uses_the_configured_route_and_preserves_the_canonical_window() {
        if let Some(listener) = utils::test_support::permitted(
            "open a loopback socket",
            tokio::net::TcpListener::bind("127.0.0.1:0").await,
        ) {
            let url = format!("http://{}/v1/", listener.local_addr().unwrap());
            let output = json!([
                {"type": "message", "role": "user", "content": "retained user requirement"},
                {"type": "compaction", "encrypted_content": "opaque", "future": {"keep": 7}}
            ]);
            let response =
                json!({"output": output, "usage": {"input_tokens": 1000, "output_tokens": 80}})
                    .to_string();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut reader = BufReader::new(socket);
                let mut path = String::new();
                reader.read_line(&mut path).await.unwrap();
                let mut line = String::new();
                let mut length = 0;
                while line != "\r\n" {
                    line.clear();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                assert!(length < 32 * 1024);
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap();
                let wire = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
                reader.get_mut().write_all(wire.as_bytes()).await.unwrap();
                CapturedRequest { path, body }
            });
            let client = OpenAIClient::new(OpenAIConfig {
                auth: OpenAIAuthConfig::Local(LocalOpenAIConfig { api_key: None, url }),
                model: "fixture".into(),
                effort: OpenAIEffort::Low,
                request_encrypted_reasoning: None,
            })
            .unwrap();
            let response = client
                .compact(
                    ClientRequest::new(vec![Message::new("task".into())])
                        .with_system("current instructions".into()),
                )
                .await
                .unwrap();
            assert_eq!(serde_json::to_value(response.output).unwrap(), output);
            assert_eq!(response.usage.input_tokens, 1000);
            let captured = server.await.unwrap();
            assert_eq!(captured.path.trim(), "POST /v1/responses/compact HTTP/1.1");
            assert_eq!(captured.body["model"], "fixture");
            assert_eq!(captured.body["instructions"], "current instructions");
            assert_eq!(
                captured.body["input"],
                json!([{"type": "message", "role": "user", "content": "task"}])
            );
            assert!(captured.body.get("stream").is_none());
            assert!(captured.body.get("store").is_none());
        }
    }
}
