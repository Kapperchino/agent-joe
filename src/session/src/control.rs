use crate::activation::SessionActivation;
use crate::persistence::SessionPersistence;
use crate::runtime::SessionRuntime;
use crate::{Event, QueuedInput, ResumableSession, SessionStore};
use analysis::contexts::context::Context;
use clients::llm::{self, LLmClient};
use commands::command::{Command, ResumeTarget};
use common_models::tui_models::{ActorToTuiPacket, SessionMessage, SessionTranscript};
use conversation::Conversation;
use turn_engine::machine::TurnMachine;
use turn_engine::turn::FollowUp;
use utils::git::worktrees::session::PruneMode;

enum SessionAction<'a> {
    List,
    Pick,
    Resume { id: &'a str },
    Current { id: &'a str },
    New,
    Fork,
    Prune(PruneMode),
}

impl<'a> SessionAction<'a> {
    fn new(
        command: &'a Command,
        turn: &TurnMachine,
        current: Option<&str>,
    ) -> anyhow::Result<Self> {
        match command {
            Command::Sessions => Ok(Self::List),
            Command::Prune(_) if !turn.is_idle() => Err(anyhow::anyhow!(
                "Interrupt the active turn before pruning worktrees"
            )),
            Command::Resume(_) | Command::New | Command::Fork if !turn.is_idle() => Err(
                anyhow::anyhow!("Interrupt the active turn before switching sessions"),
            ),
            Command::Resume(ResumeTarget::Picker) => Ok(Self::Pick),
            Command::Resume(ResumeTarget::Session { id }) if current == Some(id.as_str()) => {
                Ok(Self::Current { id })
            }
            Command::Resume(ResumeTarget::Session { id }) => Ok(Self::Resume { id }),
            Command::New => Ok(Self::New),
            Command::Fork => Ok(Self::Fork),
            Command::Prune(mode) => Ok(Self::Prune(match mode {
                commands::command::PruneMode::Merged => PruneMode::Merged,
                commands::command::PruneMode::Force => PruneMode::Force,
            })),
            _ => Err(anyhow::anyhow!("Unsupported session command")),
        }
    }
}

pub struct SessionControl<'a> {
    pub conversation: &'a mut Conversation,
    pub persistence: SessionPersistence<'a>,
}

impl SessionControl<'_> {
    pub fn append_history(&mut self, messages: Vec<llm::Message>) {
        let stored = self
            .persistence
            .session
            .map(|session| session.snapshot())
            .transpose()
            .map(|snapshot| snapshot.and_then(|snapshot| snapshot.pending));
        let messages = match stored {
            Ok(Some(batch)) => batch.messages().into(),
            Ok(None) => messages,
            Err(error) => {
                self.persistence.fail(error);
                messages
            }
        };
        self.persistence.record(Event::History(messages.clone()));
        self.conversation.append(messages);
    }

    pub fn queue_input(&mut self, input: &FollowUp) {
        if let Some(session) = self.persistence.session {
            self.persistence.record(Event::Queued(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
    }

    pub fn begin_turn(&mut self, input: FollowUp) {
        if let Some(session) = self.persistence.session {
            self.persistence.record(Event::Began(QueuedInput {
                turn: session.key(input.id),
                prompt: input.prompt.clone(),
            }));
        }
        if let Some(prompt) = input.prompt {
            self.conversation.push(llm::Message::new(prompt));
        }
    }
}

pub enum SessionCommand<C: Context> {
    Report(ActorToTuiPacket),
    Clear,
    Activate {
        activation: Box<SessionActivation<C>>,
        packet: ActorToTuiPacket,
    },
}

pub struct SessionCommands<'a, C: Context> {
    pub context: &'a C,
    pub runtime: &'a SessionRuntime,
    pub client: &'a LLmClient,
    pub turn: &'a TurnMachine,
    pub conversation: &'a Conversation,
}

impl<C: Context + Clone> SessionCommands<'_, C> {
    pub async fn run(&self, command: &Command) -> anyhow::Result<SessionCommand<C>> {
        let store = self
            .runtime
            .sessions
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Session storage is not configured"))?;
        let current = self
            .runtime
            .session
            .as_ref()
            .map(|session| session.id.as_str());
        Ok(match SessionAction::new(command, self.turn, current)? {
            SessionAction::List => SessionCommand::Report(ActorToTuiPacket::CommandResult(
                command.clone(),
                list_sessions(&store, current)?,
            )),
            SessionAction::Prune(mode) => {
                let runtime = self.runtime;
                runtime
                    .interaction
                    .authorize(tools::tool_defs::ToolEffect::Write)?;
                let project = runtime
                    .project
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("Session project is not configured"))?;
                let report =
                    tokio::task::spawn_blocking(move || store.prune_worktrees(&project, mode))
                        .await??;
                SessionCommand::Report(ActorToTuiPacket::CommandResult(command.clone(), report))
            }
            SessionAction::Pick => SessionCommand::Report(ActorToTuiPacket::SessionChoices(Ok(
                store.resume_choices(&self.client.session_provider(), current)?,
            ))),
            SessionAction::Current { id } => SessionCommand::Report(
                ActorToTuiPacket::SessionResumed(Ok(session_transcript(self.conversation, id))),
            ),
            SessionAction::Resume { id } => {
                let workspace = self
                    .runtime
                    .project
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| self.runtime.scope.workspace())?;
                let session =
                    ResumableSession::new(&store, id, &workspace, &self.client.session_provider())?
                        .resume()?;
                let activation =
                    SessionActivation::resume(self.context, self.runtime, session, None).await?;
                let packet = ActorToTuiPacket::SessionResumed(Ok(session_transcript(
                    &activation.state.conversation,
                    id,
                )));
                SessionCommand::Activate {
                    activation: Box::new(activation),
                    packet,
                }
            }
            SessionAction::New => SessionCommand::Clear,
            SessionAction::Fork => {
                let current = self
                    .runtime
                    .session
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("There is no current session"))?;
                let source = current.snapshot()?.worktree;
                let session = current.fork()?;
                let id = session.id.clone();
                let activation =
                    SessionActivation::resume(self.context, self.runtime, session, source.as_ref())
                        .await?;
                let workspace = match source {
                    Some(_) => "The conversation uses a separate Git worktree.",
                    None => {
                        "Both conversations use the same workspace; filesystem changes are shared."
                    }
                };
                SessionCommand::Activate {
                    activation: Box::new(activation),
                    packet: ActorToTuiPacket::CommandResult(
                        command.clone(),
                        format!("Forked conversation into session {id}. {workspace}"),
                    ),
                }
            }
        })
    }
}

fn session_transcript(conversation: &Conversation, id: &str) -> SessionTranscript {
    let messages = conversation
        .history()
        .iter()
        .skip(1)
        .flat_map(|message| {
            message
                .content
                .iter()
                .filter_map(|block| match (&message.role, block) {
                    (llm::Role::User, llm::ContentBlock::MessageBlock { text, .. }) => {
                        Some(SessionMessage::User(text.clone()))
                    }
                    (llm::Role::Assistant, llm::ContentBlock::MessageBlock { text, .. }) => {
                        Some(SessionMessage::Assistant(text.clone()))
                    }
                    (_, llm::ContentBlock::ToolBlock { name, input, .. }) => {
                        Some(SessionMessage::Tool(format!(
                            "{name}: {}",
                            serde_json::Value::Object(input.clone())
                        )))
                    }
                    (_, llm::ContentBlock::ToolResult { content, .. }) => {
                        Some(SessionMessage::Tool(content.clone()))
                    }
                    (_, llm::ContentBlock::ThinkingBlock { thinking, .. }) => {
                        Some(SessionMessage::Thinking(thinking.clone()))
                    }
                    (_, llm::ContentBlock::OpenAIReasoning(item)) => {
                        Some(SessionMessage::Thinking(
                            item.summary
                                .iter()
                                .map(|part| part.text.as_str())
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ))
                    }
                    (_, llm::ContentBlock::RuntimeUpdate(_)) => None,
                    (_, llm::ContentBlock::OpenAICompaction(_)) => Some(SessionMessage::Thinking(
                        "Provider-compacted context".into(),
                    )),
                })
        })
        .collect();
    SessionTranscript {
        id: id.to_owned(),
        messages,
    }
}

fn list_sessions(store: &SessionStore, current: Option<&str>) -> anyhow::Result<String> {
    let rows = store
        .list()?
        .into_iter()
        .filter(|snapshot| snapshot.parent.is_none())
        .map(|snapshot| {
            let marker = if current == Some(snapshot.id.as_str()) {
                " (current)"
            } else {
                ""
            };
            let title: String = snapshot
                .history
                .iter()
                .skip(1)
                .find_map(|message| match message.role {
                    llm::Role::User => Some(message.text()),
                    llm::Role::Assistant => None,
                })
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            format!(
                "{}  {:?}{marker}  {}",
                snapshot.id,
                snapshot.status,
                title.replace(['\n', '\r'], " ")
            )
        })
        .collect::<Vec<_>>();
    Ok(format!(
        "Sessions in this project:\n{}\nUse /resume to pick a session, /resume <id> to load one directly, or /new to start a fresh conversation.",
        rows.join("\n")
    ))
}
