use crate::event_reporter::EventReporter;
use clients::llm::StreamEvent;
use clients::response::StreamNextStep;
use response_stream::{StreamNotification, StreamProcessor};
use tokio::io::AsyncWriteExt;
use tracing::error;

pub struct StreamOutput {
    pub stream_log: Option<tokio::fs::File>,
    pub reporter: EventReporter,
}

impl StreamOutput {
    pub async fn process(
        &mut self,
        processor: &mut StreamProcessor,
        item: StreamEvent,
    ) -> anyhow::Result<StreamNextStep> {
        self.log_stream_item(&item).await;
        let update = processor.process_stream_event(item);
        for notification in update.notifications {
            self.send(notification);
        }
        update.next
    }

    pub fn send(&self, notification: StreamNotification) {
        match notification {
            StreamNotification::Packet(packet) => self.reporter.send(packet),
            StreamNotification::Usage(usage) => self.reporter.usage(usage),
        }
    }

    async fn log_stream_item(&mut self, item: &StreamEvent) {
        if let Some(file) = self.stream_log.as_mut() {
            let result = async {
                let mut line = serde_json::to_vec(item)?;
                line.push(b'\n');
                file.write_all(&line).await?;
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                error!("Failed to write stream log: {error}");
            }
        }
    }
}

pub fn finish_response(
    response: turn_engine::turn::ResponseState,
    turn: common_models::runtime_ids::TurnId,
    processor: &mut StreamProcessor,
) -> Result<turn_engine::turn::AcceptedResponse, clients::failure::Failure> {
    let completion = response.completion()?;
    let items = processor.extract_and_pre_process().map_err(|error| {
        clients::failure::Failure::new(
            clients::failure::FailureKind::InvalidInput,
            error.to_string(),
        )
    })?;
    completion.finish(turn, items)
}
