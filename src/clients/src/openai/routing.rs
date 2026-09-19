use super::{OpenAIAuthConfig, OpenAIClient};
use reqwest::header::{HeaderMap, HeaderValue};
use std::sync::{Arc, Mutex};

const TURN_STATE: &str = "x-codex-turn-state";

#[derive(Debug, Clone, Default)]
pub(super) struct CodexRouting {
    state: Arc<Mutex<RoutingState>>,
}

#[derive(Debug, Default)]
enum RoutingState {
    #[default]
    Awaiting,
    Pinned {
        session: String,
        value: HeaderValue,
    },
}

impl CodexRouting {
    fn headers(&self, session: &str) -> Result<HeaderMap, reqwest::header::InvalidHeaderValue> {
        let mut headers = HeaderMap::new();
        headers.insert("session-id", HeaderValue::from_str(session)?);
        match &*self.state.lock().unwrap() {
            RoutingState::Pinned {
                session: owner,
                value,
            } if owner == session => {
                headers.insert(TURN_STATE, value.clone());
            }
            _ => {}
        }
        Ok(headers)
    }

    fn observe(&self, session: &str, headers: &HeaderMap) {
        match (&mut *self.state.lock().unwrap(), headers.get(TURN_STATE)) {
            (state @ RoutingState::Awaiting, Some(value)) => {
                let mut value = value.clone();
                value.set_sensitive(true);
                *state = RoutingState::Pinned {
                    session: session.to_owned(),
                    value,
                };
            }
            _ => {}
        }
    }
}

impl OpenAIClient {
    pub fn begin_turn(&mut self) {
        self.routing = CodexRouting::default();
    }

    pub(super) fn routing_headers(&self, session: Option<&str>) -> super::OpenAIResult<HeaderMap> {
        match (&self.config.auth, session) {
            (OpenAIAuthConfig::Codex(_), Some(session)) => self
                .routing
                .headers(session)
                .map_err(|_| super::OpenAIError::Config("Invalid Codex session ID".into())),
            _ => Ok(HeaderMap::new()),
        }
    }

    pub(super) fn observe_routing(&self, session: Option<&str>, headers: &HeaderMap) {
        match (&self.config.auth, session) {
            (OpenAIAuthConfig::Codex(_), Some(session)) => self.routing.observe(session, headers),
            _ => {}
        }
    }
}
