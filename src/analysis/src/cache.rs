use crate::analysis::SymbolInfo;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

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
    entries: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    key: PhantomData<K>,
    value: PhantomData<V>,
}

impl<K: CacheKey, V: CacheVal> TypedCache<K, V> {
    pub fn new() -> Self {
        Self {
            entries: Arc::default(),
            key: PhantomData,
            value: PhantomData,
        }
    }

    pub fn transaction<F, R>(&mut self, f: F) -> anyhow::Result<R>
    where
        F: FnOnce(&mut TypedCacheDb<'_, K, V>) -> anyhow::Result<R>,
    {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache lock poisoned"))?;
        let mut pending = entries.clone();
        let result = f(&mut TypedCacheDb {
            entries: &mut pending,
            key: PhantomData,
            value: PhantomData,
        })?;
        *entries = pending;
        Ok(result)
    }

    pub fn read_transaction<F, R>(&self, f: F) -> anyhow::Result<R>
    where
        F: FnOnce(&TypedCacheDbRo<'_, V>) -> anyhow::Result<R>,
    {
        let entries = self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("Cache lock poisoned"))?;
        f(&TypedCacheDbRo {
            entries: &entries,
            value: PhantomData,
        })
    }
}

pub struct TypedCacheDb<'a, K: CacheKey, V: CacheVal> {
    entries: &'a mut BTreeMap<String, Vec<u8>>,
    key: PhantomData<K>,
    value: PhantomData<V>,
}

pub struct TypedCacheDbRo<'a, V: CacheVal> {
    entries: &'a BTreeMap<String, Vec<u8>>,
    value: PhantomData<V>,
}

pub struct CacheEntry<V> {
    pub key: String,
    pub value: V,
}

impl<K: CacheKey, V: CacheVal> TypedCacheDb<'_, K, V> {
    pub fn put(&mut self, key: &K, value: &V) -> anyhow::Result<()> {
        self.entries
            .insert(key.get_key(), serde_json::to_vec(value)?);
        Ok(())
    }

    pub fn delete(&mut self, key: &K) -> anyhow::Result<()> {
        self.delete_string_key(&key.get_key())
    }

    pub fn delete_string_key(&mut self, key: &str) -> anyhow::Result<()> {
        self.entries.remove(key);
        Ok(())
    }

    pub fn prefix_iter(
        &self,
        prefix: String,
    ) -> anyhow::Result<impl Iterator<Item = CacheEntry<V>>> {
        let values = self
            .entries
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(key, value)| {
                Ok(CacheEntry {
                    key: key.clone(),
                    value: serde_json::from_slice(value)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(values.into_iter())
    }
}

impl<V: CacheVal> TypedCacheDbRo<'_, V> {
    pub fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.entries.is_empty())
    }

    pub fn iter(&self) -> anyhow::Result<impl Iterator<Item = V>> {
        let values = self
            .entries
            .values()
            .map(|value| serde_json::from_slice(value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values.into_iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl CacheKey for String {
        fn get_key(&self) -> String {
            self.clone()
        }
    }
    impl CacheVal for String {}

    #[test]
    fn transactions_share_commits_and_discard_failed_changes() {
        let mut cache = TypedCache::<String, String>::new();
        let shared = cache.clone();
        cache
            .transaction(|db| db.put(&"key".into(), &"original".into()))
            .unwrap();
        let result: anyhow::Result<()> = cache.transaction(|db| {
            db.put(&"key".into(), &"changed".into())?;
            Err(anyhow::anyhow!("failed transaction"))
        });
        assert!(result.is_err());
        assert_eq!(
            shared
                .read_transaction(|db| Ok(db.iter()?.collect::<Vec<_>>()))
                .unwrap(),
            vec!["original"]
        );
    }
}
