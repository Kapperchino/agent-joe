use super::{SchemaVersion, Snapshot, artifact_index::ArtifactIndex};
use anyhow::Context;
use heed::{
    Database, Env, EnvOpenOptions,
    types::{Bytes, Str},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufReader, Read},
    ops::Deref,
    sync::{Arc, Mutex, MutexGuard},
};
use utils::workspace::{PrivateStorage, WorkspacePolicy};

const MANIFEST: &str = "generations.json";
const DEFAULT_MAP_SIZE: u64 = 100 * 1024 * 1024 * 1024;
static OPEN: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct Capacity {
    map_size: usize,
    rotate_at: usize,
}

impl Capacity {
    fn new(map_size: u64) -> anyhow::Result<Self> {
        let map_size =
            usize::try_from(map_size).context("Session storage requires a 64-bit address space")?;
        match map_size >= 1024 * 1024 && map_size.is_multiple_of(64 * 1024) {
            true => Ok(Self {
                map_size,
                rotate_at: map_size / 10 * 9,
            }),
            false => Err(anyhow::anyhow!(
                "Session map size must be at least 1 MiB and a multiple of 64 KiB"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredLayout")]
struct Layout {
    current: u64,
    old: Option<u64>,
}

#[derive(Deserialize)]
struct StoredLayout {
    current: u64,
    old: Option<u64>,
}

impl Layout {
    fn new(current: u64, old: Option<u64>) -> anyhow::Result<Self> {
        match current.checked_sub(1) == old {
            true => Ok(Self { current, old }),
            false => Err(anyhow::anyhow!("Invalid session database generations")),
        }
    }

    fn next(self) -> anyhow::Result<Self> {
        let current = self
            .current
            .checked_add(1)
            .context("Session database generation overflow")?;
        Self::new(current, Some(self.current))
    }
}

impl TryFrom<StoredLayout> for Layout {
    type Error = anyhow::Error;

    fn try_from(stored: StoredLayout) -> anyhow::Result<Self> {
        Self::new(stored.current, stored.old)
    }
}

#[derive(Serialize, Deserialize)]
enum StorageState {
    Ready(Layout),
    Retiring { layout: Layout, archived: u64 },
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: SchemaVersion,
    state: StorageState,
}

impl Manifest {
    fn new(state: StorageState) -> anyhow::Result<Self> {
        match state {
            StorageState::Retiring { layout, archived }
                if archived.checked_add(2) != Some(layout.current) =>
            {
                Err(anyhow::anyhow!(
                    "Invalid retired session database generation"
                ))
            }
            state => Ok(Self {
                version: SchemaVersion,
                state,
            }),
        }
    }
}

impl StorageState {
    fn load(storage: &PrivateStorage) -> anyhow::Result<Self> {
        let state = match storage.read_file(MANIFEST)? {
            Some(file) => serde_json::from_reader::<_, Manifest>(file.take(4096))?.state,
            None => Self::Ready(Layout::default()),
        };
        Manifest::new(state).map(|manifest| manifest.state)
    }

    fn save(self, storage: &PrivateStorage) -> anyhow::Result<()> {
        storage.replace_file(MANIFEST, &serde_json::to_vec(&Manifest::new(self)?)?)
    }
}

pub struct SessionStore {
    pub(super) storage: Arc<PrivateStorage>,
    capacity: Capacity,
    catalog: Mutex<Option<Catalog>>,
}

pub(super) struct SessionDatabase {
    pub env: Env,
    pub snapshots: Database<Str, Bytes>,
    pub events: Database<Str, Bytes>,
    pub owners: Database<Str, Bytes>,
    pub artifacts: Database<Str, Bytes>,
    pub artifact_index: ArtifactIndex,
    pub storage: Arc<PrivateStorage>,
}

impl SessionDatabase {
    fn open(storage: Arc<PrivateStorage>, capacity: Capacity) -> anyhow::Result<Self> {
        let _guard = OPEN
            .lock()
            .map_err(|_| anyhow::anyhow!("Session storage initialization lock poisoned"))?;
        for name in ["data.mdb", "lock.mdb"] {
            storage.open_file(name)?.sync_all()?;
        }
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(capacity.map_size)
                .max_dbs(5)
                .open(storage.path())?
        };
        let mut transaction = env.write_txn()?;
        let snapshots = env.create_database(&mut transaction, Some("session_snapshots"))?;
        let events = env.create_database(&mut transaction, Some("session_events"))?;
        let owners = env.create_database(&mut transaction, Some("session_owners"))?;
        let artifacts = env.create_database(&mut transaction, Some("session_artifacts"))?;
        let artifact_index = ArtifactIndex::open(&env, &mut transaction, snapshots)?;
        transaction.commit()?;
        storage.sync()?;
        Ok(Self {
            env,
            snapshots,
            events,
            owners,
            artifacts,
            artifact_index,
            storage,
        })
    }

    fn used_bytes(&self) -> usize {
        (self.env.info().last_page_number + 1) * self.env.stat().page_size as usize
    }

    fn contains(&self, id: &str) -> anyhow::Result<bool> {
        let transaction = self.env.read_txn()?;
        Ok(self.snapshots.get(&transaction, id)?.is_some())
    }

    fn copy_session(
        &self,
        destination: &Self,
        transaction: &mut heed::RwTxn<'_>,
        id: &str,
    ) -> anyhow::Result<()> {
        let source = self.env.read_txn()?;
        let mut snapshot = self.snapshot(&source, id)?;
        snapshot.artifacts = self.artifact_index.list(&source, id)?;
        for artifact in &snapshot.artifacts {
            if destination
                .artifacts
                .get(transaction, &artifact.id)?
                .is_none()
            {
                let content = self
                    .artifacts
                    .get(&source, &artifact.id)?
                    .ok_or_else(|| anyhow::anyhow!("Artifact {} is missing", artifact.id))?;
                destination
                    .artifacts
                    .put(transaction, &artifact.id, content)?;
            }
        }
        destination
            .artifact_index
            .inherit(transaction, id, &snapshot.artifacts)?;
        destination
            .snapshots
            .put(transaction, id, &serde_json::to_vec(&snapshot)?)?;
        if let Some(owner) = self.owners.get(&source, id)? {
            destination.owners.put(transaction, id, owner)?;
        }
        for entry in self.events.prefix_iter(&source, &format!("{id}:"))? {
            let (key, value) = entry?;
            destination.events.put(transaction, key, value)?;
        }
        Ok(())
    }
}

pub(super) struct Catalog {
    layout: Layout,
    pub current: SessionDatabase,
    old: Option<SessionDatabase>,
}

impl Catalog {
    fn open(
        storage: &Arc<PrivateStorage>,
        capacity: Capacity,
        layout: Layout,
    ) -> anyhow::Result<Self> {
        let open = |id| {
            let directory = generation(storage, id)?;
            directory
                .read_file("data.mdb")?
                .with_context(|| format!("Session database generation {id} is missing"))?;
            SessionDatabase::open(directory, capacity)
        };
        Ok(Self {
            layout,
            current: open(layout.current)?,
            old: layout.old.map(open).transpose()?,
        })
    }

    pub fn database(&self, id: &str) -> anyhow::Result<&SessionDatabase> {
        match (self.current.contains(id)?, &self.old) {
            (true, _) => Ok(&self.current),
            (false, Some(old)) if old.contains(id)? => Ok(old),
            _ => Err(anyhow::anyhow!(
                "Session {id} does not exist in the current or old database; archived sessions are not searchable"
            )),
        }
    }

    fn sessions(&self) -> anyhow::Result<BTreeMap<String, Snapshot>> {
        self.old.iter().chain([&self.current]).try_fold(
            BTreeMap::new(),
            |mut sessions, database| {
                sessions.extend(
                    database
                        .list()?
                        .into_iter()
                        .map(|snapshot| (snapshot.id.clone(), snapshot)),
                );
                Ok(sessions)
            },
        )
    }

    fn migrate(&self, id: Option<&str>) -> anyhow::Result<()> {
        match id {
            None => Ok(()),
            Some(id) if self.current.contains(id)? => Ok(()),
            Some(id) => {
                self.database(id)?;
                let family = Conversation::new(&self.sessions()?, [id])?;
                let mut transaction = self.current.env.write_txn()?;
                family.sessions.iter().try_for_each(|id| {
                    match self.current.snapshots.get(&transaction, id)? {
                        Some(_) => Ok(()),
                        None => {
                            self.database(id)?
                                .copy_session(&self.current, &mut transaction, id)
                        }
                    }
                })?;
                transaction.commit()?;
                Ok(())
            }
        }
    }

    fn write<T>(
        &self,
        id: Option<&str>,
        action: &impl Fn(&SessionDatabase) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.migrate(id)?;
        action(&self.current)
    }

    fn rotate(
        &self,
        storage: &Arc<PrivateStorage>,
        capacity: Capacity,
        requested: Option<&str>,
    ) -> anyhow::Result<Layout> {
        let layout = self.layout.next()?;
        let next = layout.current;
        tracing::info!(
            current = self.layout.current,
            next,
            "Rotating session storage"
        );
        remove_generation(storage, next)?;
        let destination = SessionDatabase::open(generation(storage, next)?, capacity)?;
        let sessions = self.sessions()?;
        let mut active = BTreeSet::new();
        for id in sessions.keys() {
            let database = self.database(id)?;
            let transaction = database.env.read_txn()?;
            if let Some(owner) = database.owner(&transaction, id)?
                && owner.is_running()?
            {
                active.insert(id.as_str());
            }
        }
        active.extend(requested);
        let family = Conversation::new(&sessions, active)?;
        for id in family.sessions {
            let mut transaction = destination.env.write_txn()?;
            self.database(&id)?
                .copy_session(&destination, &mut transaction, &id)
                .context("Moving active conversations to the next session database")?;
            transaction.commit()?;
        }
        match destination.used_bytes() < capacity.rotate_at {
            false => Err(anyhow::anyhow!(
                "Active conversations fill the next session database; the existing databases were retained"
            )),
            true => {
                destination.env.force_sync()?;
                match self.layout.old {
                    Some(archived) => {
                        archive(storage, archived)?;
                        StorageState::Retiring { layout, archived }.save(storage)?;
                    }
                    None => StorageState::Ready(layout).save(storage)?,
                }
                Ok(layout)
            }
        }
    }
}

struct Conversation {
    sessions: BTreeSet<String>,
}

impl Conversation {
    fn new<'a>(
        sessions: &BTreeMap<String, Snapshot>,
        selected: impl IntoIterator<Item = &'a str>,
    ) -> anyhow::Result<Self> {
        let root = |id: &str| -> anyhow::Result<String> {
            let mut current = Some(id.to_owned());
            let mut visited = BTreeSet::new();
            let mut root = id.to_owned();
            while let Some(id) = current {
                match visited.insert(id.clone()) {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!("Cycle in session parent links")),
                }?;
                let snapshot = sessions
                    .get(&id)
                    .ok_or_else(|| anyhow::anyhow!("Session {id} does not exist"))?;
                current = snapshot.parent.clone();
                root = id;
            }
            Ok(root)
        };
        let roots = selected
            .into_iter()
            .map(root)
            .collect::<anyhow::Result<BTreeSet<_>>>()?;
        let mut included = BTreeSet::new();
        for id in sessions.keys() {
            if roots.contains(&root(id)?) {
                included.insert(id.clone());
            }
        }
        Ok(Self { sessions: included })
    }
}

fn generation(storage: &Arc<PrivateStorage>, id: u64) -> anyhow::Result<Arc<PrivateStorage>> {
    match id {
        0 => Ok(storage.clone()),
        _ => storage.child(&format!("generation-{id:020}")).map(Arc::new),
    }
}

fn remove_generation(storage: &Arc<PrivateStorage>, id: u64) -> anyhow::Result<()> {
    let directory = generation(storage, id)?;
    for file in ["data.mdb", "lock.mdb"] {
        directory.remove_file(file)?;
    }
    if id != 0 {
        storage.remove_child(&format!("generation-{id:020}"))?;
    }
    Ok(())
}

fn archive(storage: &Arc<PrivateStorage>, id: u64) -> anyhow::Result<()> {
    let source = generation(storage, id)?
        .read_file("data.mdb")?
        .context("The old session database is missing")?;
    let name = format!("archive-{id:020}.mdb.zst");
    let temporary = format!("{name}.tmp");
    let output = storage.open_file(&temporary)?;
    output.set_len(0)?;
    let mut encoder = zstd::stream::write::Encoder::new(output, 3)?;
    encoder.include_checksum(true)?;
    let bytes = std::io::copy(&mut BufReader::new(source), &mut encoder)?;
    encoder.finish()?.sync_all()?;
    let file = storage
        .read_file(&temporary)?
        .context("Session archive is missing")?;
    let mut decoder = zstd::stream::read::Decoder::new(file)?;
    let decoded = std::io::copy(&mut decoder, &mut std::io::sink())?;
    match bytes == decoded {
        true => storage.publish_file(&temporary, &name),
        false => Err(anyhow::anyhow!("Session archive verification failed")),
    }
}

pub(super) struct StoreAccess<'a> {
    catalog: MutexGuard<'a, Option<Catalog>>,
    _lock: File,
}

impl Deref for StoreAccess<'_> {
    type Target = Catalog;

    fn deref(&self) -> &Self::Target {
        self.catalog
            .as_ref()
            .expect("Session catalog is open under the storage lock")
    }
}

impl StoreAccess<'_> {
    fn reload(&mut self, store: &SessionStore) -> anyhow::Result<()> {
        *self.catalog = None;
        let state = StorageState::load(&store.storage)?;
        let layout = match state {
            StorageState::Ready(layout) => layout,
            StorageState::Retiring { layout, archived } => {
                store
                    .storage
                    .read_file(&format!("archive-{archived:020}.mdb.zst"))?
                    .context("Session archive is missing; retaining the old database")?;
                remove_generation(&store.storage, archived)?;
                StorageState::Ready(layout).save(&store.storage)?;
                layout
            }
        };
        *self.catalog = Some(Catalog::open(&store.storage, store.capacity, layout)?);
        Ok(())
    }

    fn rotate(&mut self, store: &SessionStore, id: Option<&str>) -> anyhow::Result<()> {
        Catalog::rotate(self, &store.storage, store.capacity, id)?;
        self.reload(store)
    }
}

enum WriteState {
    Current,
    Rotating,
}

impl WriteState {
    fn new(used_bytes: usize, capacity: Capacity) -> Self {
        match used_bytes >= capacity.rotate_at {
            true => Self::Rotating,
            false => Self::Current,
        }
    }

    fn commit<T>(
        self,
        access: &mut StoreAccess<'_>,
        store: &SessionStore,
        id: Option<&str>,
        action: &impl Fn(&SessionDatabase) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        match self {
            Self::Current => match access.write(id, action) {
                Err(error)
                    if matches!(
                        error.downcast_ref::<heed::Error>(),
                        Some(heed::Error::Mdb(heed::MdbError::MapFull))
                    ) =>
                {
                    Self::Rotating.commit(access, store, id, action)
                }
                result => result,
            },
            Self::Rotating => {
                access.rotate(store, id)?;
                access.write(id, action)
            }
        }
    }
}

impl SessionStore {
    pub fn open(workspace: &WorkspacePolicy, namespace: &str) -> anyhow::Result<Arc<Self>> {
        Self::open_with_capacity(workspace, namespace, Capacity::new(DEFAULT_MAP_SIZE)?)
    }

    fn open_with_capacity(
        workspace: &WorkspacePolicy,
        namespace: &str,
        capacity: Capacity,
    ) -> anyhow::Result<Arc<Self>> {
        let storage = {
            let _guard = OPEN
                .lock()
                .map_err(|_| anyhow::anyhow!("Session storage initialization lock poisoned"))?;
            workspace
                .session_storage(namespace)
                .context("Creating private session storage")?
        };
        let store = Arc::new(Self {
            storage: Arc::new(storage),
            capacity,
            catalog: Mutex::new(None),
        });
        drop(store.access()?);
        Ok(store)
    }

    pub(super) fn access(&self) -> anyhow::Result<StoreAccess<'_>> {
        let catalog = self
            .catalog
            .lock()
            .map_err(|_| anyhow::anyhow!("Session storage lock poisoned"))?;
        let lock = self.storage.open_file("rotation.lock")?;
        lock.lock()?;
        let mut access = StoreAccess {
            catalog,
            _lock: lock,
        };
        let state = StorageState::load(&self.storage)?;
        let fresh = matches!(state, StorageState::Ready(layout) if access.catalog.as_ref().is_some_and(|catalog| catalog.layout == layout));
        if !fresh {
            access.reload(self)?;
        }
        Ok(access)
    }

    pub(super) fn read<T>(
        &self,
        id: &str,
        action: impl FnOnce(&SessionDatabase) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let access = self.access()?;
        action(access.database(id)?)
    }

    pub(super) fn update<T>(
        &self,
        id: Option<&str>,
        action: impl Fn(&SessionDatabase) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut access = self.access()?;
        WriteState::new(access.current.used_bytes(), self.capacity).commit(
            &mut access,
            self,
            id,
            &action,
        )
    }

    pub fn list(&self) -> anyhow::Result<Vec<Snapshot>> {
        Ok(self.access()?.sessions()?.into_values().collect())
    }
}

#[cfg(test)]
#[path = "../tests/unit/session/generations/tests.rs"]
mod tests;
