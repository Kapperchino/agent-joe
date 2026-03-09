#[cfg(test)]
mod tests {
    use clients::llm::StreamEvent;
    use std::path::{Path, PathBuf};
    use tokio::io::{AsyncBufReadExt, BufReader};

    async fn load_stream_log(path: &Path) -> Vec<StreamEvent> {
        let file = tokio::fs::File::open(path).await.expect("failed to open stream log");
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut events = Vec::new();

        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<StreamEvent>(&line) {
                Ok(event) => events.push(event),
                Err(e) => {
                    eprintln!("skipping line: {e}");
                }
            }
        }

        events
    }

    #[tokio::test]
    async fn test_replay_stream_log() {
        let mut log_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        log_path.push("resources/stream_replay_1.jsonl");
        if !log_path.exists() {
            eprintln!("no stream log found at {:?}, skipping test", log_path);
            return;
        }
        let log_path = log_path.as_path();

        let events = load_stream_log(log_path).await;
        assert!(!events.is_empty(), "stream log should contain events");

        println!("loaded {} stream events from log", events.len());

        for (i, event) in events.iter().enumerate() {
            println!("event {}: {:?}", i, std::mem::discriminant(event));
        }
    }

    #[tokio::test]
    async fn test_deserialize_stream_events() {
        let samples = vec![
            r#"{"MessageStart":{"message":{"id":"resp_123","model":"gpt-5.4","role":"Assistant","usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":0}}}}"#,
            r#"{"ContentBlockStart":{"index":0,"content_block":{"Text":{"text":""}}}}"#,
            r#"{"ContentBlockDelta":{"index":0,"delta":{"TextDelta":{"text":"Hello world"}}}}"#,
            r#"{"ContentBlockStop":{"index":0}}"#,
            r#"{"MessageDelta":{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}}"#,
            r#""MessageStop""#,
        ];

        for (i, json) in samples.iter().enumerate() {
            let event: StreamEvent =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("failed to deserialize sample {i}: {e}\njson: {json}"));
            println!("sample {i}: {:?}", event);
        }
    }
}