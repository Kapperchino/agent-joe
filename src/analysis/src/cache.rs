use crate::analysis::SymbolInfo;
use anyhow::Context;
use heed::types::{SerdeJson, Str};
use heed::{Database, Env, EnvOpenOptions, RoTxn, RwTxn};
use std::path::PathBuf;
use tokio::fs;

const CACHE_MAP_SIZE: usize = 1024 * 1024 * 1024;

pub trait CacheKey {
    fn get_key(&self) -> String;
}
impl CacheKey for SymbolInfo {
    fn get_key(&self) -> String {
        let container = self.container_name.as_deref().unwrap_or("self");
        format!("{}-{}-{:?}-{}", self.rpath, container, self.kind, self.name)
    }
}
pub trait CacheVal: serde::Serialize + for<'de> serde::Deserialize<'de> {}

impl CacheVal for SymbolInfo {}

#[derive(Clone)]
pub struct TypedCache<K: CacheKey, V: CacheVal> {
    env: Env,
    _p_key: std::marker::PhantomData<K>,
    _p_val: std::marker::PhantomData<V>,
}

impl<K: CacheKey + 'static, V: CacheVal + 'static> TypedCache<K, V> {
    pub async fn new(path: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&path)
            .await
            .with_context(|| format!("Failed to create cache directory {}", path.display()))?;
        let env = unsafe { EnvOpenOptions::new().map_size(CACHE_MAP_SIZE).open(&path) }
            .with_context(|| format!("Failed to open cache {}", path.display()))?;
        let mut wtxn = env.write_txn()?;
        env.create_database::<Str, SerdeJson<V>>(&mut wtxn, None)?;
        wtxn.commit()?;

        Ok(Self {
            env,
            _p_key: std::marker::PhantomData,
            _p_val: std::marker::PhantomData,
        })
    }

    pub fn transaction<F, R>(&mut self, f: F) -> anyhow::Result<R>
    where
        F: for<'txn, 'env> FnOnce(&mut TypedCacheDb<'txn, 'env, K, V>) -> anyhow::Result<R>,
    {
        let mut wtxn = self.env.write_txn()?;
        let res = {
            let db: Database<Str, SerdeJson<V>> = self.env.create_database(&mut wtxn, None)?;
            let mut cache_db = TypedCacheDb {
                db,
                txn: &mut wtxn,
                _marker: std::marker::PhantomData,
            };
            f(&mut cache_db)?
        };
        wtxn.commit()?;
        Ok(res)
    }

    pub fn read_transaction<F, R>(&self, f: F) -> anyhow::Result<R>
    where
        F: for<'txn, 'env> FnOnce(&TypedCacheDbRo<'txn, 'env, K, V>) -> anyhow::Result<R>,
    {
        let rtxn = self.env.read_txn()?;
        let db: Database<Str, SerdeJson<V>> = self
            .env
            .open_database(&rtxn, None)?
            .ok_or_else(|| anyhow::anyhow!("Database not found"))?;
        let cache_db = TypedCacheDbRo {
            db,
            txn: &rtxn,
            _marker: std::marker::PhantomData,
        };
        f(&cache_db)
    }
}

pub struct TypedCacheDb<'txn, 'env, K: CacheKey, V: CacheVal> {
    db: Database<Str, SerdeJson<V>>,
    txn: &'txn mut RwTxn<'env>,
    _marker: std::marker::PhantomData<K>,
}

pub struct TypedCacheDbRo<'txn, 'env, K: CacheKey, V: CacheVal> {
    db: Database<Str, SerdeJson<V>>,
    txn: &'txn RoTxn<'env>,
    _marker: std::marker::PhantomData<K>,
}

impl<K: CacheKey, V: CacheVal> TypedCacheDb<'_, '_, K, V> {
    pub fn put(&mut self, key: &K, val: &V) -> anyhow::Result<()> {
        self.db.put(self.txn, &key.get_key(), val)?;
        Ok(())
    }

    pub fn delete(&mut self, key: &K) -> anyhow::Result<()> {
        self.db.delete(self.txn, &key.get_key())?;
        Ok(())
    }

    pub fn delete_string_key(&mut self, key: &str) -> anyhow::Result<()> {
        self.db.delete(self.txn, key)?;
        Ok(())
    }

    pub fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.db.is_empty(self.txn)?)
    }

    pub fn iter(&self) -> anyhow::Result<impl Iterator<Item = V>> {
        Ok(self
            .db
            .iter(self.txn)?
            .filter_map(|r| r.ok().map(|(_, v)| v)))
    }

    pub fn prefix_iter(&self, key: String) -> anyhow::Result<impl Iterator<Item = (String, V)>> {
        Ok(self
            .db
            .prefix_iter(self.txn, &key)?
            .filter_map(|r| r.ok().map(|(k, v)| (k.to_string(), v))))
    }

    pub fn get(&self, key: &str) -> anyhow::Result<Option<V>> {
        Ok(self.db.get(self.txn, key)?)
    }
}

impl<K: CacheKey, V: CacheVal> TypedCacheDbRo<'_, '_, K, V> {
    pub fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.db.is_empty(self.txn)?)
    }

    pub fn iter(&self) -> anyhow::Result<impl Iterator<Item = V>> {
        Ok(self
            .db
            .iter(self.txn)?
            .filter_map(|r| r.ok().map(|(_, v)| v)))
    }

    pub fn prefix_iter(&self, key: String) -> anyhow::Result<impl Iterator<Item = (String, V)>> {
        Ok(self
            .db
            .prefix_iter(self.txn, &key)?
            .filter_map(|r| r.ok().map(|(k, v)| (k.to_string(), v))))
    }

    pub fn get(&self, key: &str) -> anyhow::Result<Option<V>> {
        Ok(self.db.get(self.txn, key)?)
    }
}
