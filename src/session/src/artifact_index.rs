use super::{Snapshot, artifacts::ArtifactReference};
use heed::{
    Database, Env, RoTxn, RwTxn,
    types::{Bytes, Str},
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

const DATABASE: &str = "conversation_artifacts";

pub(super) struct ArtifactIndex {
    entries: Database<Str, Bytes>,
}

impl ArtifactIndex {
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
                let sessions = snapshots
                    .iter(transaction)?
                    .map(|entry| {
                        let (id, bytes) = entry?;
                        Ok((id.to_owned(), super::decode::<ArtifactSession>(bytes)?))
                    })
                    .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
                for (id, session) in &sessions {
                    let lineage = ArtifactLineage::new(id, |id| {
                        sessions
                            .get(id)
                            .map(|session| session.parent.clone())
                            .ok_or_else(|| anyhow::anyhow!("Session {id} does not exist"))
                    })?;
                    for owner in lineage.sessions {
                        index.inherit(transaction, &owner, &session.artifacts)?;
                    }
                }
                Ok(index)
            }
        }
    }

    pub fn inherit(
        &self,
        transaction: &mut RwTxn<'_>,
        session: &str,
        artifacts: &[ArtifactReference],
    ) -> anyhow::Result<()> {
        artifacts.iter().try_for_each(|artifact| {
            let key = format!("{session}:{}", artifact.id);
            self.entries
                .put(transaction, &key, &serde_json::to_vec(artifact)?)?;
            Ok(())
        })
    }

    pub fn record(
        &self,
        transaction: &mut RwTxn<'_>,
        snapshots: Database<Str, Bytes>,
        snapshot: &Snapshot,
        artifact: &ArtifactReference,
    ) -> anyhow::Result<()> {
        let lineage = ArtifactLineage::new(&snapshot.id, |id| {
            snapshots
                .get(transaction, id)?
                .ok_or_else(|| anyhow::anyhow!("Session {id} does not exist"))
                .and_then(super::decode::<ArtifactSession>)
                .map(|session| session.parent)
        })?;
        lineage.sessions.iter().try_for_each(|session| {
            self.inherit(transaction, session, std::slice::from_ref(artifact))
        })
    }

    pub fn list(
        &self,
        transaction: &RoTxn<'_>,
        session: &str,
    ) -> anyhow::Result<Vec<ArtifactReference>> {
        self.entries
            .prefix_iter(transaction, &format!("{session}:"))?
            .map(|entry| {
                let (_, bytes) = entry?;
                serde_json::from_slice(bytes).map_err(Into::into)
            })
            .collect()
    }

    pub fn get(
        &self,
        transaction: &RoTxn<'_>,
        session: &str,
        artifact: &str,
    ) -> anyhow::Result<ArtifactReference> {
        let bytes = self
            .entries
            .get(transaction, &format!("{session}:{artifact}"))?
            .ok_or_else(|| {
                anyhow::anyhow!("Artifact {artifact} is not part of this conversation")
            })?;
        serde_json::from_slice(bytes).map_err(Into::into)
    }
}

#[derive(Deserialize)]
struct ArtifactSession {
    parent: Option<String>,
    #[serde(default)]
    artifacts: Vec<ArtifactReference>,
}

struct ArtifactLineage {
    sessions: BTreeSet<String>,
}

impl ArtifactLineage {
    fn new(
        session: &str,
        parent: impl Fn(&str) -> anyhow::Result<Option<String>>,
    ) -> anyhow::Result<Self> {
        let mut sessions = BTreeSet::new();
        let mut next = Some(session.to_owned());
        while let Some(id) = next {
            next = match sessions.insert(id.clone()) {
                true => parent(&id),
                false => Err(anyhow::anyhow!("Cycle in session parent links")),
            }?;
        }
        Ok(Self { sessions })
    }
}
