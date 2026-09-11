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
