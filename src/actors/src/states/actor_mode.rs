use crate::actor::{ActorContext, Dependency};
use crate::event_reporter::EventReporter;
use crate::states::runtime::{ExecutionRole, Runtime};
use analysis::contexts::context::Context;
use clients::response::RequestMode;
use tools::tool_defs::erased_tool;

pub enum ActorMode {
    Conversation,
    SingleResponse(EventReporter),
    Snapshot(clients::llm::ClientRequest),
}

impl ActorMode {
    pub(crate) fn configure<C: Context + Clone + 'static>(
        &self,
        dependency: Dependency<C>,
    ) -> Dependency<C> {
        match self {
            Self::SingleResponse(_) | Self::Snapshot(_) => Dependency {
                tools: Vec::new(),
                runtime: Runtime {
                    sessions: None,
                    session: None,
                    ..dependency.runtime
                },
                ..dependency
            },
            Self::Conversation => {
                let interaction_tools = matches!(dependency.runtime.role, ExecutionRole::Root).then(|| {
                    [
                        erased_tool::<
                            crate::tools::request_user_input::RequestUserInput,
                            C,
                            ActorContext<C>,
                        >(),
                        erased_tool::<crate::tools::update_plan::UpdatePlan, C, ActorContext<C>>(),
                    ]
                });
                let artifact_tool = match (
                    &dependency.runtime.sessions,
                    dependency.tool("read_artifact"),
                ) {
                    (Some(_), None) if dependency.runtime.role.allows_tool("read_artifact") => {
                        Some(erased_tool::<
                            crate::tools::read_artifact::ReadArtifact,
                            C,
                            ActorContext<C>,
                        >())
                    }
                    _ => None,
                };
                Dependency {
                    tools: dependency
                        .tools
                        .into_iter()
                        .chain(interaction_tools.into_iter().flatten())
                        .chain(artifact_tool)
                        .collect(),
                    ..dependency
                }
            }
        }
    }

    pub(crate) fn request_mode(&self) -> RequestMode {
        match self {
            Self::Conversation => RequestMode::Continue,
            Self::SingleResponse(_) | Self::Snapshot(_) => RequestMode::SingleResponse,
        }
    }

    pub(crate) fn frozen_request(&self) -> Option<clients::llm::ClientRequest> {
        match self {
            Self::Snapshot(request) => Some(request.clone()),
            Self::Conversation | Self::SingleResponse(_) => None,
        }
    }

    pub(crate) fn reporter<C: Context>(self, dependency: &Dependency<C>) -> EventReporter {
        match self {
            Self::Conversation | Self::Snapshot(_) => EventReporter::Interactive {
                actor_id: dependency.context.get_id(),
                tui_tx: dependency.tui_tx.clone(),
            },
            Self::SingleResponse(reporter) => reporter,
        }
    }
}
