use crate::actor::Message;
use anyhow::Context as _;
use ractor::{Actor, ActorRef, RpcReplyPort};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

#[derive(Debug)]
pub enum ImmutableMessage {
    Ask {
        question: String,
        reply: RpcReplyPort<anyhow::Result<String>>,
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
    pub async fn spawn<A: Actor<Msg = ImmutableMessage>>(
        worker: A,
        arguments: A::Arguments,
        description: ImmutableWorkerDescription,
        owner: &ActorRef<Message>,
    ) -> anyhow::Result<Self> {
        let (actor, handle) =
            Actor::spawn_linked(None, worker, arguments, owner.get_cell()).await?;
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

#[derive(Default)]
pub struct ImmutableWorkerRegistry {
    workers: Mutex<BTreeMap<String, Vec<ImmutableWorker>>>,
}

impl ImmutableWorkerRegistry {
    pub fn insert(&self, owner: &str, worker: ImmutableWorker) -> ImmutableWorkerView {
        let view = worker.endpoint.view.clone();
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(owner.to_owned())
            .or_default()
            .push(worker);
        view
    }

    pub fn list(&self, owner: &str) -> Vec<ImmutableWorkerView> {
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
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
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(owner)
            .into_iter()
            .flatten()
            .find(|worker| worker.endpoint.view.worker_id == worker_id)
            .map(|worker| worker.endpoint.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("Unknown immutable worker {worker_id} in this conversation")
            })?;
        let (reply, receive) = tokio::sync::oneshot::channel();
        endpoint.actor.send_message(ImmutableMessage::Ask {
            question,
            reply: reply.into(),
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
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(owner)
            .unwrap_or_default();
        for worker in workers {
            worker.stop().await;
        }
    }
}
