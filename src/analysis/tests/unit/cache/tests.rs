use super::*;

impl CacheKey for String {
    fn get_key(&self) -> String {
        self.clone()
    }
}
impl CacheVal for String {}

#[test]
fn cache_iteration_and_prefix_results_remain_sorted_by_key() {
    let mut cache = TypedCache::<String, String>::new();
    cache
        .transaction(|db| {
            for key in ["prefix-z", "other", "prefix-a", "prefix-m"] {
                db.put(&key.into(), &key.into())?;
            }
            let matches = db
                .prefix_iter("prefix-".into())?
                .map(|entry| entry.key)
                .collect::<Vec<_>>();
            assert_eq!(matches, ["prefix-a", "prefix-m", "prefix-z"]);
            Ok(())
        })
        .unwrap();
    let values = cache
        .read_transaction(|db| Ok(db.iter()?.collect::<Vec<_>>()))
        .unwrap();
    assert_eq!(values, ["other", "prefix-a", "prefix-m", "prefix-z"]);
}

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
