use crate::actor::Message;
use crate::worker::{Worker, WorkerAdapter};
use anyhow::Context as _;
use ractor::{Actor, ActorRef, RpcReplyPort};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Debug)]
pub enum ImmutableMessage {
    Ask {
        question: String,
        reply: RpcReplyPort<anyhow::Result<String>>,
        admission: Option<tokio::sync::OwnedSemaphorePermit>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmutableWorkerDescription {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImmutableWorkerView {
    pub worker_id: String,
    #[serde(flatten)]
    pub description: ImmutableWorkerDescription,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImmutableAnswer {
    pub worker: ImmutableWorkerView,
    pub answer: String,
}

#[derive(Clone)]
struct Endpoint {
    view: ImmutableWorkerView,
    actor: ActorRef<ImmutableMessage>,
}

pub struct ImmutableWorker {
    endpoint: Endpoint,
    handle: tokio::task::JoinHandle<()>,
}

impl ImmutableWorker {
    pub(crate) fn view(&self) -> ImmutableWorkerView {
        self.endpoint.view.clone()
    }

    pub async fn spawn<W: Worker<Msg = ImmutableMessage>>(
        worker: W,
        arguments: W::Arguments,
        description: ImmutableWorkerDescription,
        owner: &ActorRef<Message>,
    ) -> anyhow::Result<Self> {
        let (actor, handle) = Actor::spawn_linked(
            None,
            WorkerAdapter::new(worker),
            arguments,
            owner.get_cell(),
        )
        .await?;
        Ok(Self {
            endpoint: Endpoint {
                view: ImmutableWorkerView {
                    worker_id: uuid::Uuid::new_v4().to_string(),
                    description,
                },
                actor,
            },
            handle,
        })
    }

    async fn stop(mut self) {
        self.endpoint.actor.kill();
        let _ = (&mut self.handle).await;
    }
}

impl Drop for ImmutableWorker {
    fn drop(&mut self) {
        self.endpoint.actor.kill();
    }
}

pub struct ImmutableWorkerRegistry {
    pub(crate) state: Mutex<RegistryState>,
    pub(crate) knowledge_gate: Arc<tokio::sync::Semaphore>,
    pub(crate) knowledge_queue: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
pub(crate) struct RegistryState {
    pub(crate) workers: BTreeMap<String, Vec<ImmutableWorker>>,
    pub(crate) knowledge: BTreeMap<String, crate::knowledge::Slot>,
}

impl Default for ImmutableWorkerRegistry {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            knowledge_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            knowledge_queue: Arc::new(tokio::sync::Semaphore::new(16)),
        }
    }
}

impl ImmutableWorkerRegistry {
    pub fn insert(&self, owner: &str, worker: ImmutableWorker) -> ImmutableWorkerView {
        let view = worker.endpoint.view.clone();
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .workers
            .entry(owner.to_owned())
            .or_default()
            .push(worker);
        view
    }

    pub fn list(&self, owner: &str) -> Vec<ImmutableWorkerView> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .workers
            .get(owner)
            .into_iter()
            .flatten()
            .map(|worker| worker.endpoint.view.clone())
            .collect()
    }

    pub async fn ask(
        &self,
        owner: &str,
        worker_id: &str,
        question: String,
        timeout: Duration,
    ) -> anyhow::Result<ImmutableAnswer> {
        let endpoint = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .workers
            .get(owner)
            .into_iter()
            .flatten()
            .find(|worker| worker.endpoint.view.worker_id == worker_id)
            .map(|worker| worker.endpoint.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("Unknown immutable worker {worker_id} in this conversation")
            })?;
        let admission = match endpoint.view.description.kind.as_str() {
            "knowledge" => {
                match question.len() <= 16384 {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!(
                        "Knowledge questions are limited to 16384 bytes"
                    )),
                }?;
                Some(
                    self.knowledge_queue
                        .clone()
                        .try_acquire_owned()
                        .context("At most sixteen knowledge questions may be queued")?,
                )
            }
            _ => None,
        };
        let (reply, receive) = tokio::sync::oneshot::channel();
        endpoint.actor.send_message(ImmutableMessage::Ask {
            question,
            reply: reply.into(),
            admission,
        })?;
        let answer = tokio::time::timeout(timeout, receive)
            .await
            .context("Immutable worker question timed out")?
            .context("Immutable worker stopped before answering")??;
        Ok(ImmutableAnswer {
            worker: endpoint.view,
            answer,
        })
    }

    pub async fn clear(&self, owner: &str) {
        let workers = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(slot) = state.knowledge.remove(owner) {
                slot.invalidate("Conversation cleared or closed");
            }
            state.workers.remove(owner).unwrap_or_default()
        };
        for worker in workers {
            worker.stop().await;
        }
    }

    pub fn clear_knowledge(&self, owner: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(slot) = state.knowledge.remove(owner) {
            slot.invalidate("Knowledge generation retired");
        }
        if let Some(workers) = state.workers.get_mut(owner) {
            workers.retain(|worker| worker.endpoint.view.description.kind != "knowledge");
        }
    }
}
