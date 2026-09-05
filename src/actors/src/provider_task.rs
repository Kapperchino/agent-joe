use crate::{
    actor::Message,
    turn::{ProviderRun, Tag},
};
use clients::{
    failure::{Failure, FailureKind},
    llm::{ClientRequest, LLmClient, StreamEvent},
};
use futures::{FutureExt, StreamExt};
use ractor::ActorRef;
use std::{panic::AssertUnwindSafe, time::Duration};
use utils::execution::{ExecutionScope, ResourceKind};

#[derive(Debug)]
pub enum ProviderEvent {
    Item(StreamEvent),
    Finished(Result<(), Failure>),
}

pub fn spawn(
    actor: ActorRef<Message>,
    client: LLmClient,
    request: ClientRequest,
    run: &ProviderRun,
    owner: &ExecutionScope,
    previous: Option<ExecutionScope>,
    timeout: Duration,
) {
    let scope = run.scope.clone();
    let tag = run.tag;
    let attempt = run.attempt;
    let task = scope.tasks.clone().spawn(async move {
        let _registration = scope.register(ResourceKind::Provider, format!("Turn {} request {}", tag.turn, tag.operation));
        if let Some(previous) = previous { previous.finish().await; }
        tokio::select! {
            biased;
            _ = scope.cancel.cancelled() => {},
            result = AssertUnwindSafe(pump(&actor, tag, client, request, attempt, timeout)).catch_unwind() => {
                let result = result.unwrap_or_else(|_| Err(Failure::new(FailureKind::Transport, "Provider task panicked")));
                let _ = actor.send_message(Message::Provider { tag, event: ProviderEvent::Finished(result) });
            }
        }
    });
    owner.tasks.spawn(async move {
        let _ = task.await;
    });
}

async fn pump(
    actor: &ActorRef<Message>,
    tag: Tag,
    mut client: LLmClient,
    request: ClientRequest,
    attempt: u8,
    timeout: Duration,
) -> Result<(), Failure> {
    if attempt > 0 {
        tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
    }
    let response = tokio::time::timeout(timeout, client.chat_stream(request))
        .await
        .map_err(|_| Failure::new(FailureKind::Transport, "Provider request timed out"))
        .and_then(|result| result.map_err(Failure::from_error));
    match response {
        Ok(mut stream) => loop {
            match tokio::time::timeout(timeout, stream.next()).await {
                Ok(Some(Ok(event))) => {
                    if actor
                        .send_message(Message::Provider {
                            tag,
                            event: ProviderEvent::Item(event),
                        })
                        .is_err()
                    {
                        break Ok(());
                    }
                }
                Ok(Some(Err(error))) => break Err(Failure::from_error(error)),
                Ok(None) => break Ok(()),
                Err(_) => {
                    break Err(Failure::new(
                        FailureKind::Transport,
                        "Provider stream timed out",
                    ));
                }
            }
        },
        Err(failure) => Err(failure),
    }
}
