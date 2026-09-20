use crate::{ResumableSession, Session, SessionStore};
use clients::llm::SessionProvider;
use std::{path::PathBuf, sync::Arc};
use utils::workspace::WorkspacePolicy;

pub struct Workspace {
    pub path: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "joe-m4-{}-{}",
            std::process::id(),
            common_models::runtime_ids::OperationId::new()
        ));
        std::fs::create_dir(&path).unwrap();
        Self { path }
    }

    pub fn store(&self) -> Arc<SessionStore> {
        open(&self.path)
    }
    pub fn resume(
        &self,
        store: &Arc<SessionStore>,
        id: &str,
        provider: &SessionProvider,
    ) -> anyhow::Result<Arc<Session>> {
        let policy = WorkspacePolicy::workspace(self.path.clone())?;
        ResumableSession::new(store, id, &policy, provider)?.resume()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

pub(crate) fn open(path: &std::path::Path) -> Arc<SessionStore> {
    SessionStore::open(
        &WorkspacePolicy::workspace(path.to_owned()).unwrap(),
        "sessions",
    )
    .unwrap()
}

pub fn invalidate(store: &SessionStore, id: &str) {
    let access = store.access().unwrap();
    let database = &access.current;
    let mut transaction = database.env.write_txn().unwrap();
    database
        .snapshots
        .put(&mut transaction, id, br#"{"version":999}"#)
        .unwrap();
    transaction.commit().unwrap();
    drop(access);
}
