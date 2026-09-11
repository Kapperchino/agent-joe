use super::*;
use futures::StreamExt;
async fn events(text: &str) -> Vec<anyhow::Result<crate::llm::StreamEvent>> {
    let stream = futures::stream::iter(
        text.as_bytes()
            .iter()
            .map(|byte| Ok::<_, std::io::Error>(vec![*byte]))
            .collect::<Vec<_>>(),
    );
    decode(stream, |event: &crate::claude::StreamEvent| {
        matches!(
            event,
            crate::claude::StreamEvent::MessageStop | crate::claude::StreamEvent::Error { .. }
        )
    })
    .map(|result| result.map(Into::into))
    .collect()
    .await
}
#[tokio::test]
async fn claude_terminal_errors_keepalives_and_premature_eof() {
    let result = events(": keepalive\r\n\r\ndata:{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"busy\"}}\r\n\r\n").await;
    assert!(matches!(
        result.as_slice(),
        [Ok(crate::llm::StreamEvent::Error { .. })]
    ));
    assert!(
        events("data: {\"type\":\"ping\"}\n\ndata: [DONE]\n\n")
            .await
            .last()
            .unwrap()
            .is_err()
    );
    assert!(events("data: {\"type\":\"message_stop\"}\n\n").await[0].is_ok());
}
#[tokio::test]
async fn openai_terminal_failure_is_not_swallowed_by_keepalives() {
    let bytes = b": ping\n\ndata:{\"type\":\"error\",\"code\":\"rate_limit_exceeded\",\"message\":\"slow down\"}\n\n";
    let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(bytes.to_vec())]);
    let result = decode(stream, |event: &crate::openai::StreamEvent| {
        matches!(event, crate::openai::StreamEvent::Error { .. })
    })
    .collect::<Vec<_>>()
    .await;
    assert!(
        matches!(result.as_slice(), [Ok(crate::openai::StreamEvent::Error { code, .. })] if code == "rate_limit_exceeded")
    );
}
