#[derive(Default)]
pub(crate) struct Decoder {
    line: Vec<u8>,
    data: Vec<String>,
    after_cr: bool,
    size: usize,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> anyhow::Result<Vec<String>> {
        bytes.iter().try_fold(Vec::new(), |mut events, &byte| {
            self.push_byte(byte).map(|event| {
                events.extend(event);
                events
            })
        })
    }

    fn push_byte(&mut self, byte: u8) -> anyhow::Result<Option<String>> {
        match (self.after_cr, byte) {
            (true, b'\n') => {
                self.after_cr = false;
                Ok(None)
            }
            (_, b'\r' | b'\n') => {
                self.after_cr = byte == b'\r';
                self.finish_line()
            }
            _ => {
                self.after_cr = false;
                self.line.push(byte);
                self.size += 1;
                if self.size <= 16 * 1024 * 1024 {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("SSE event exceeds 16 MiB"))
                }
            }
        }
    }

    fn finish_line(&mut self) -> anyhow::Result<Option<String>> {
        String::from_utf8(std::mem::take(&mut self.line))
            .map_err(anyhow::Error::from)
            .map(|line| {
                if line.is_empty() {
                    self.size = 0;
                    if self.data.is_empty() {
                        None
                    } else {
                        Some(std::mem::take(&mut self.data).join("\n"))
                    }
                } else {
                    if let Some(value) = line.strip_prefix("data:") {
                        self.data
                            .push(value.strip_prefix(' ').unwrap_or(value).to_owned());
                    }
                    None
                }
            })
    }

    pub fn finish(&self) -> anyhow::Result<()> {
        if self.line.is_empty() && self.data.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Premature EOF inside SSE event"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_framing_and_keepalives_at_every_chunk_boundary() {
        let input = ": ping\r\n\r\nevent: msg\r\ndata:{\"text\":\"終😀\",\r\ndata: \"ok\":true}\r\n\r\ndata: [DONE]\n\n";
        for split in 0..=input.len() {
            let mut decoder = Decoder::default();
            let mut events = decoder.push(&input.as_bytes()[..split]).unwrap();
            events.extend(decoder.push(&input.as_bytes()[split..]).unwrap());
            decoder.finish().unwrap();
            assert_eq!(events, ["{\"text\":\"終😀\",\n\"ok\":true}", "[DONE]"]);
        }
    }
    #[test]
    fn rejects_partial_event_and_invalid_utf8() {
        let mut decoder = Decoder::default();
        decoder.push(b"data: {}\n").unwrap();
        assert!(decoder.finish().is_err());
        assert!(Decoder::default().push(b"data: \xff\n\n").is_err());
    }
}

#[derive(Default)]
enum StreamState {
    #[default]
    Reading,
    Completed,
    Failed,
}

pub(crate) fn decode<T, S, B, E>(
    bytes: S,
    terminal: fn(&T) -> bool,
) -> impl futures::Stream<Item = anyhow::Result<T>> + Send
where
    T: serde::de::DeserializeOwned + Send + 'static,
    S: futures::Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send,
    E: Into<anyhow::Error> + Send,
{
    use futures::StreamExt;
    async_stream::stream! {
        futures::pin_mut!(bytes);
        let mut decoder = Decoder::default();
        let mut state = StreamState::Reading;
        while let Some(chunk) = bytes.next().await {
            match chunk.map_err(Into::into).and_then(|chunk| decoder.push(chunk.as_ref())) {
                Ok(events) => {
                    for data in events.into_iter().filter(|data| data != "[DONE]") {
                        match serde_json::from_str::<T>(&data) {
                            Ok(event) => {
                                if terminal(&event) { state = StreamState::Completed; }
                                yield Ok(event);
                            }
                            Err(error) => {
                                state = StreamState::Failed;
                                yield Err(crate::failure::Failure::new(crate::failure::FailureKind::InvalidInput, error.to_string()).into());
                            }
                        }
                        if !matches!(state, StreamState::Reading) { break; }
                    }
                }
                Err(error) => {
                    state = StreamState::Failed;
                    yield Err(error);
                }
            }
            if !matches!(state, StreamState::Reading) { break; }
        }
        if matches!(state, StreamState::Reading) {
            yield decoder.finish().and(Err(crate::failure::Failure::new(
                crate::failure::FailureKind::Transport, "Provider stream ended before its terminal event").into()));
        }
    }
}

#[cfg(test)]
mod stream_tests {
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
}
