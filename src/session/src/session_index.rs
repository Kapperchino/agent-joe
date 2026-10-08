use super::{SchemaVersion, Snapshot};
use clients::llm::SessionProvider;
use common_models::tui_models::SessionSummary;
use heed::{
    Database, Env, RoTxn, RwTxn,
    types::{Bytes, Str},
};
use serde::{Deserialize, Serialize};

const DATABASE: &str = "session_summaries";

pub(super) struct SessionIndex {
    entries: Database<Str, Bytes>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct SessionListing {
    version: SchemaVersion,
    pub id: String,
    pub provider: SessionProvider,
    pub summary: Option<SessionSummary>,
}

impl SessionListing {
    fn new(snapshot: &Snapshot) -> Self {
        Self {
            version: SchemaVersion,
            id: snapshot.id.clone(),
            provider: snapshot.provider.clone(),
            summary: snapshot
                .parent
                .is_none()
                .then(|| snapshot.summary())
                .flatten(),
        }
    }
}

impl SessionIndex {
    pub fn open(
        env: &Env,
        transaction: &mut RwTxn<'_>,
        snapshots: Database<Str, Bytes>,
    ) -> anyhow::Result<Self> {
        match env.open_database(transaction, Some(DATABASE))? {
            Some(entries) => Ok(Self { entries }),
            None => {
                let index = Self {
                    entries: env.create_database(transaction, Some(DATABASE))?,
                };
                let listings = snapshots
                    .iter(transaction)?
                    .map(|entry| {
                        let (_, bytes) = entry?;
                        let snapshot: Snapshot = serde_json::from_slice(bytes)?;
                        Ok(SessionListing::new(&snapshot))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                listings.iter().try_for_each(|listing| {
                    index
                        .entries
                        .put(transaction, &listing.id, &serde_json::to_vec(listing)?)
                        .map_err(anyhow::Error::from)
                })?;
                Ok(index)
            }
        }
    }

    pub fn record(&self, transaction: &mut RwTxn<'_>, snapshot: &Snapshot) -> anyhow::Result<()> {
        self.entries.put(
            transaction,
            &snapshot.id,
            &serde_json::to_vec(&SessionListing::new(snapshot))?,
        )?;
        Ok(())
    }

    pub fn list(&self, transaction: &RoTxn<'_>) -> anyhow::Result<Vec<SessionListing>> {
        self.entries
            .iter(transaction)?
            .map(|entry| {
                let (_, bytes) = entry?;
                serde_json::from_slice(bytes).map_err(Into::into)
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests/unit/session_index_test.rs"]
mod tests;
