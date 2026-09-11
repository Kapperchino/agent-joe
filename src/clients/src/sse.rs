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
#[path = "../tests/unit/sse/tests.rs"]
mod tests;

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
#[path = "../tests/unit/sse/stream_tests.rs"]
mod stream_tests;
