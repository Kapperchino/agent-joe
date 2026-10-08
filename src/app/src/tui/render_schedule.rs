use common_models::tui_models::ActorToTui;
use crossterm::event::Event;
use flume::Receiver;
use futures::{Stream, StreamExt};
use std::time::Duration;
use tokio::time::Instant;

const FRAME_INTERVAL: Duration = Duration::from_millis(34);

pub(super) enum UiEvent {
    Render,
    Terminal(Event),
    Actor(ActorToTui),
    Closed,
}

enum RenderState {
    Idle,
    Scheduled(Instant),
}

pub(super) struct RenderSchedule {
    state: RenderState,
    last_frame: Option<Instant>,
}

impl RenderSchedule {
    pub(super) fn new() -> Self {
        Self {
            state: RenderState::Scheduled(Instant::now()),
            last_frame: None,
        }
    }

    pub(super) fn request(&mut self) {
        let now = Instant::now();
        let deadline = self
            .last_frame
            .map(|last| (last + FRAME_INTERVAL).max(now))
            .unwrap_or(now);
        self.state = match self.state {
            RenderState::Idle => RenderState::Scheduled(deadline),
            RenderState::Scheduled(current) => RenderState::Scheduled(current.min(deadline)),
        };
    }

    pub(super) fn rendered(&mut self, animation: Option<Instant>) {
        let now = Instant::now();
        self.last_frame = Some(now);
        self.state = match animation {
            Some(deadline) => RenderState::Scheduled(deadline.max(now + FRAME_INTERVAL)),
            None => RenderState::Idle,
        };
    }

    async fn wait(&self) {
        match self.state {
            RenderState::Idle => std::future::pending().await,
            RenderState::Scheduled(deadline) => tokio::time::sleep_until(deadline).await,
        }
    }

    pub(super) async fn next_event(
        &self,
        events: &mut (impl Stream<Item = std::io::Result<Event>> + Unpin),
        actor_rx: &Receiver<ActorToTui>,
    ) -> color_eyre::Result<UiEvent> {
        tokio::select! {
            biased;
            () = self.wait() => Ok(UiEvent::Render),
            event = async {
                tokio::select! {
                    event = events.next() => Ok(event.transpose()?.map(UiEvent::Terminal).unwrap_or(UiEvent::Closed)),
                    message = actor_rx.recv_async() => Ok(message.map(UiEvent::Actor).unwrap_or(UiEvent::Closed)),
                }
            } => event,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tui/render_schedule.rs"]
mod tests;
