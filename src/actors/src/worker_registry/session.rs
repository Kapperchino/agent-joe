use crate::session::Session;
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};
use worker_registry::report::StoredWorkerEvidence;

#[derive(Default)]
pub struct WorkerSession {
    state: Mutex<SessionState>,
}

#[derive(Default)]
enum SessionState {
    #[default]
    Detached,
    Attached {
        session: Arc<Session>,
        inherited_artifacts: BTreeSet<String>,
    },
}

impl WorkerSession {
    pub fn attach(&self, session: Option<Arc<Session>>) -> anyhow::Result<()> {
        let state = match session {
            Some(session) => SessionState::Attached {
                inherited_artifacts: session
                    .snapshot()?
                    .artifacts
                    .into_iter()
                    .map(|artifact| artifact.id)
                    .collect(),
                session,
            },
            None => SessionState::Detached,
        };
        *self.state.lock().unwrap() = state;
        Ok(())
    }

    pub fn evidence(&self) -> StoredWorkerEvidence {
        match &*self.state.lock().unwrap() {
            SessionState::Detached => StoredWorkerEvidence::default(),
            SessionState::Attached {
                session,
                inherited_artifacts,
            } => match session.snapshot() {
                Ok(snapshot) => StoredWorkerEvidence {
                    artifacts: snapshot
                        .artifacts
                        .into_iter()
                        .filter(|artifact| !inherited_artifacts.contains(&artifact.id))
                        .collect(),
                    processes: snapshot.processes.into_values().collect(),
                    unresolved_issues: Vec::new(),
                },
                Err(error) => StoredWorkerEvidence {
                    unresolved_issues: vec![format!(
                        "Could not retrieve worker session evidence: {error}"
                    )],
                    ..Default::default()
                },
            },
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/worker_session.rs"]
mod tests;
