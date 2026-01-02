use crate::analysis::SymbolInfo;
use heed::types::{SerdeJson, Str};
use heed::{Database, Env, EnvOpenOptions, RwTxn};
use std::io::ErrorKind;
use std::path::PathBuf;
use tokio::fs;

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

pub struct TypedCache<K: CacheKey, V: CacheVal> {
    env: Env,
    _p_key: std::marker::PhantomData<K>,
    _p_val: std::marker::PhantomData<V>,
}

impl<K: CacheKey, V: CacheVal + 'static> TypedCache<K, V> {
    pub async fn new(path: Option<PathBuf>) -> Self {
        let path = path.unwrap_or("/Users/kamranorhun/.turbo-code/".into());
        match fs::create_dir(path.clone()).await {
            Ok(_) => Ok(()),
            Err(err) => {
                if err.kind() != ErrorKind::AlreadyExists {
                    Err(err)
                } else {
                    Ok(())
                }
            }
        }
        .unwrap();
        let env = unsafe { EnvOpenOptions::new().open(&path) }.unwrap();
        Self {
            env,
            _p_key: std::marker::PhantomData,
            _p_val: std::marker::PhantomData,
        }
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
}

pub struct TypedCacheDb<'txn, 'env, K: CacheKey, V: CacheVal> {
    db: Database<Str, SerdeJson<V>>,
    txn: &'txn mut RwTxn<'env>,
    _marker: std::marker::PhantomData<K>,
}

impl<K: CacheKey, V: CacheVal> TypedCacheDb<'_, '_, K, V> {
    pub fn put(&mut self, key: &K, val: &V) -> anyhow::Result<()> {
        self.db.put(self.txn, &key.get_key(), val)?;
        Ok(())
    }

    pub fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.db.is_empty(self.txn)?)
    }

    pub fn iter(&self) -> anyhow::Result<impl Iterator<Item = V> > {
        Ok(self
            .db
            .iter(self.txn)?
            .filter_map(|r| r.ok().map(|(_, v)| v)))
    }

    pub fn get(&self, key: &str) -> anyhow::Result<Option<V>> {
        Ok(self.db.get(self.txn, key)?)
    }
}
