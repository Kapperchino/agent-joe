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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
