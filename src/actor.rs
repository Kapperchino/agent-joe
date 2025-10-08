use anyhow::{Error, anyhow};
use log::{info, log};
use ractor::ActorProcessingErr;
use ractor::ActorRef;
use ractor::{Actor, MessagingErr};

const INITIAL_PROMPT: &str = "You are an agent making code changes to a rust codebase";

// Base unit for the agent, should be given context and then simply do the work
pub struct Worker {}

#[derive(Debug, Clone)]
pub enum Message {
    Init,
    StartWork(),
    Done,
}

pub struct ActorState {
    cur_state: State,
    prompts: Vec<String>,
}

pub enum State {
    Init,
    Working,
    Stopped,
}
impl Message {}

#[cfg_attr(feature = "async-trait", ractor::async_trait)]
impl Actor for Worker {
    type Msg = Message;
    type State = ActorState;
    type Arguments = ();

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        _: (),
    ) -> Result<Self::State, ActorProcessingErr> {
        // startup the event processing
        myself.send_message(Message::Init).unwrap();
        Ok(ActorState {
            cur_state: State::Init,
            prompts: vec![INITIAL_PROMPT.to_string()],
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match state.cur_state {
            State::Init => match message {
                Message::Init => myself
                    .send_message(Message::StartWork())
                    .map_err(|err| anyhow!(err)),
                Message::StartWork() => {
                    info!("Start working boy");
                    state.cur_state = State::Working;
                    Ok(())
                }
                _ => Ok(()),
            },
            State::Working => Ok(()),
            State::Stopped => Ok(()),
        }?;

        Ok(())
    }
}

pub async fn run() {
    let (_, actor_handle) = Actor::spawn(None, Worker {}, ())
        .await
        .expect("Failed to start actor");
    actor_handle.await.expect("Actor failed to exit cleanly");
}
