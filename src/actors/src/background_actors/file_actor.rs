use analysis::{
    contexts::{context::Context, rust_context::RustContext},
    rust_proj::RustProject,
};
use anyhow::anyhow;
use notify_types::event::EventKind;
use ractor::{Actor, ActorCell, ActorProcessingErr, ActorRef, call};
use ractor_actors::filewatcher::{
    FileWatcher, FileWatcherConfig, FileWatcherMessage, FileWatcherSubscriber, SubscriptionResult,
};
use std::{path::PathBuf, time::Duration};
use utils::workspace::Access;

pub struct FileActor;

pub struct Dependency {
    pub context: RustContext,
    pub scope: utils::execution::ExecutionScope,
}

pub struct FileActorState {
    context: RustContext,
    scope: utils::execution::ExecutionScope,
    watcher: ActorRef<FileWatcherMessage>,
    refresh: Refresh,
}

enum Refresh {
    Clean,
    Dirty,
}

#[derive(Debug)]
pub enum Message {
    Changed(Vec<PathBuf>),
    ApplyVFS,
}

impl Actor for FileActor {
    type Msg = Message;
    type State = FileActorState;
    type Arguments = Dependency;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        dependency: Dependency,
    ) -> Result<Self::State, ActorProcessingErr> {
        let config = FileWatcherConfig {
            directories: vec![dependency.context.cur_dir.clone()],
            files: Vec::new(),
        };
        let (watcher, _) =
            Actor::spawn_linked(None, FileWatcher, config, myself.get_cell()).await?;
        match call!(watcher, |reply| FileWatcherMessage::Subscribe(
            myself.get_id(),
            Box::new(Forwarder {
                actor: myself.get_cell()
            }),
            reply
        ))? {
            SubscriptionResult::Ok => Ok(()),
            _ => Err(anyhow!("Could not subscribe to workspace changes")),
        }?;
        myself.send_interval(Duration::from_secs(1), || Message::ApplyVFS);
        Ok(FileActorState {
            context: dependency.context,
            scope: dependency.scope,
            watcher,
            refresh: Refresh::Dirty,
        })
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            Message::Changed(paths) => {
                if paths
                    .iter()
                    .any(|path| relevant(path, &state.context.rust_proj))
                {
                    state.refresh = Refresh::Dirty;
                }
            }
            Message::ApplyVFS => match state.refresh {
                Refresh::Clean => {}
                Refresh::Dirty => {
                    match state.scope.enter(state.context.refresh_workspace()).await {
                        Ok(()) => state.refresh = Refresh::Clean,
                        Err(error) => tracing::warn!("Workspace refresh failed: {error}"),
                    }
                }
            },
        }
        Ok(())
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        state.scope.finish().await;
        state.watcher.stop_and_wait(None, None).await?;
        Ok(())
    }
}

fn relevant(path: &std::path::Path, project: &RustProject) -> bool {
    project
        .workspace()
        .relative_path(path, Access::Read)
        .is_ok_and(|path| {
            path != std::path::Path::new("logs/err.log")
                && !path.components().any(|component| {
                    [".git", "target"]
                        .iter()
                        .any(|name| component.as_os_str().eq_ignore_ascii_case(name))
                })
        })
}

struct Forwarder {
    actor: ActorCell,
}

impl FileWatcherSubscriber for Forwarder {
    fn event_received(&self, event: notify_types::event::Event) {
        if matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
        ) {
            let _ = self.actor.send_message(Message::Changed(event.paths));
        }
    }
}

pub async fn start(
    context: &RustContext,
    owner: &ActorRef<crate::actor::Message>,
) -> Result<ActorRef<Message>, ActorProcessingErr> {
    let (actor, _) = Actor::spawn_linked(
        None,
        FileActor,
        Dependency {
            context: context.clone(),
            scope: utils::execution::ExecutionScope::current().child(),
        },
        owner.get_cell(),
    )
    .await?;
    Ok(actor)
}
