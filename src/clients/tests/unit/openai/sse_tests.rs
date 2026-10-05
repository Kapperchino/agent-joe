use super::*;
use crate::FastMode;
use crate::config::{Config, ConfigContext};
use crate::llm::LLmClient;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct CapturedRequest {
    path: String,
    headers: header::HeaderMap,
    body: serde_json::Value,
    encoded: String,
}

struct RequestCase {
    client: LLmClient,
    tier: Option<&'static str>,
}

#[tokio::test]
async fn sse_snapshots_refresh_fast_settings_and_yield_text_before_completion() {
    let fixture = crate::http_fixture::HttpFixture::new().await;
    let mut client = fixture.client(config(codex_auth()));
    let url = fixture.url.clone();
    client.client = ClientBuilder::from_client(client.client)
        .with_init(move |builder: reqwest_middleware::RequestBuilder| {
            let (transport, request) = builder.build_split();
            let mut request = request.unwrap();
            assert_eq!(request.url().host_str(), Some("chatgpt.com"));
            *request.url_mut() = format!("{url}{}", request.url().path()).parse().unwrap();
            reqwest_middleware::RequestBuilder::from_parts(transport, request)
        })
        .build();
    let mut root = LLmClient::OpenApi {
        client,
        config: ConfigContext::new(Config::OpenAI(config(codex_auth()))),
    };
    let active = root.snapshot();
    let mut enabled = config(codex_auth());
    enabled.fast_mode = FastMode::Enabled;
    match &mut root {
        LLmClient::OpenApi { config, .. } => {
            *config = ConfigContext::new(Config::OpenAI(enabled));
        }
        _ => panic!("Expected an OpenAI client"),
    }
    let next = root.snapshot();

    for RequestCase { mut client, tier } in [
        RequestCase {
            client: next,
            tier: Some("priority"),
        },
        RequestCase {
            client: active,
            tier: None,
        },
    ] {
        let listener = fixture.listener.clone();
        let (release, wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut path = String::new();
            reader.read_line(&mut path).await.unwrap();
            let mut headers = header::HeaderMap::new();
            let mut line = String::new();
            while line != "\r\n" {
                line.clear();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                match line.split_once(':') {
                    Some((name, value)) => {
                        headers.append(
                            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                            header::HeaderValue::from_str(value.trim()).unwrap(),
                        );
                    }
                    None => {}
                }
            }
            let length = headers[header::CONTENT_LENGTH]
                .to_str()
                .unwrap()
                .parse::<usize>()
                .unwrap();
            assert!(length < 64 * 1024);
            let mut bytes = vec![0; length];
            reader.read_exact(&mut bytes).await.unwrap();
            let encoded = String::from_utf8(bytes).unwrap();
            let body = serde_json::from_str(&encoded).unwrap();
            reader
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n")
                .await
                .unwrap();
            wait.await.unwrap();
            reader
                .get_mut()
                .write_all(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"response-1\",\"model\":\"fixture\",\"status\":\"completed\",\"service_tier\":\"default\",\"output\":[]}}\n\n")
                .await
                .unwrap();
            CapturedRequest {
                path,
                headers,
                body,
                encoded,
            }
        });
        let mut stream = client
            .chat_stream(
                llm::ClientRequest::new(vec![llm::Message::new("task ".repeat(4096))])
                    .with_prompt_cache_key(Some("session-1".into())),
            )
            .await
            .unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            llm::StreamEvent::ContentBlockDelta {
                delta: llm::Delta::TextDelta { text }, ..
            } if text == "hello"
        ));
        release.send(()).unwrap();
        assert!(stream.collect::<Vec<_>>().await.iter().all(Result::is_ok));
        let captured = server.await.unwrap();
        assert_eq!(
            captured.path.trim(),
            "POST /backend-api/codex/responses HTTP/1.1"
        );
        assert_eq!(captured.headers[header::ACCEPT], "text/event-stream");
        assert_eq!(captured.headers["session-id"], "session-1");
        assert_eq!(captured.body["model"], "fixture");
        assert_eq!(captured.body["reasoning"]["effort"], "low");
        assert_eq!(captured.body["stream"], true);
        match tier {
            Some(tier) => {
                assert_eq!(captured.body["service_tier"], tier);
                assert_eq!(
                    captured.headers["x-codex-routing-hint"],
                    "model=fixture;tier=priority"
                );
                assert!(captured.encoded.starts_with(
                    "{\"model\":\"fixture\",\"stream\":true,\"service_tier\":\"priority\","
                ));
            }
            None => {
                assert!(captured.body.get("service_tier").is_none());
                assert_eq!(captured.headers["x-codex-routing-hint"], "model=fixture");
                assert!(
                    captured
                        .encoded
                        .starts_with("{\"model\":\"fixture\",\"stream\":true,")
                );
            }
        }
    }
}
