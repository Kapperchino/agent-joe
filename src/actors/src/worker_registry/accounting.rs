use clients::llm::{ClientRequest, StreamEvent};
use worker_registry::budget::{RequestReservation, TokenUsage, UsageUpdate};

pub(crate) fn reservation(request: &ClientRequest) -> anyhow::Result<RequestReservation> {
    Ok(RequestReservation {
        estimated_input_tokens: conversation::context::estimated_tokens(request)?,
        output_tokens: request.max_output_tokens.unwrap_or_default() as usize,
    })
}

pub(crate) fn usage_update(event: &StreamEvent) -> UsageUpdate {
    match event {
        StreamEvent::MessageStart { message } => UsageUpdate::Progress(TokenUsage {
            input_tokens: (message.usage.input_tokens as usize)
                .saturating_add(message.usage.cache_creation_input_tokens as usize)
                .saturating_add(message.usage.cache_read_input_tokens as usize),
            output_tokens: message.usage.output_tokens as usize,
        }),
        StreamEvent::MessageDelta { delta, usage } => {
            let tokens = TokenUsage {
                input_tokens: usage.input_tokens as usize,
                output_tokens: usage.output_tokens as usize,
            };
            match delta.stop_reason {
                Some(_) => UsageUpdate::Completed(tokens),
                None => UsageUpdate::Progress(tokens),
            }
        }
        _ => UsageUpdate::Unreported,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/worker_accounting.rs"]
mod tests;
