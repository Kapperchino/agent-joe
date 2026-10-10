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

#[test]
fn transactions_commit_deletions_and_roll_back_multi_key_changes() {
    let mut cache = TypedCache::<String, String>::new();
    cache
        .transaction(|db| {
            db.put(&"first".into(), &"first".into())?;
            db.put(&"second".into(), &"second".into())
        })
        .unwrap();
    let failed: anyhow::Result<()> = cache.transaction(|db| {
        db.delete(&"first".into())?;
        db.put(&"second".into(), &"changed".into())?;
        db.put(&"third".into(), &"third".into())?;
        Err(anyhow::anyhow!("discard batch"))
    });
    assert!(failed.is_err());
    assert_eq!(
        cache
            .read_transaction(|db| Ok(db.iter()?.collect::<Vec<_>>()))
            .unwrap(),
        ["first", "second"]
    );
    cache
        .transaction(|db| {
            db.delete_string_key("first")?;
            db.delete(&"second".into())
        })
        .unwrap();
    assert!(cache.read_transaction(|db| db.is_empty()).unwrap());
}

#[test]
fn concurrent_transactions_do_not_lose_committed_batches() {
    let cache = TypedCache::<String, String>::new();
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let threads = (0..16)
        .map(|index| {
            let mut cache = cache.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                cache
                    .transaction(|db| {
                        db.put(&format!("{index:02}-a"), &format!("{index:02}-a"))?;
                        std::thread::yield_now();
                        db.put(&format!("{index:02}-b"), &format!("{index:02}-b"))
                    })
                    .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    let expected = (0..16)
        .flat_map(|index| [format!("{index:02}-a"), format!("{index:02}-b")])
        .collect::<Vec<_>>();
    assert_eq!(
        cache
            .read_transaction(|db| Ok(db.iter()?.collect::<Vec<_>>()))
            .unwrap(),
        expected
    );
}

#[test]
fn concurrent_read_transactions_observe_complete_batches() {
    let mut cache = TypedCache::<String, String>::new();
    cache
        .transaction(|db| {
            db.put(&"first".into(), &"0".into())?;
            db.put(&"second".into(), &"0".into())
        })
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let writer_barrier = barrier.clone();
    let mut writer = cache.clone();
    let thread = std::thread::spawn(move || {
        writer_barrier.wait();
        for index in 1..100 {
            writer
                .transaction(|db| {
                    db.put(&"first".into(), &index.to_string())?;
                    std::thread::yield_now();
                    db.put(&"second".into(), &index.to_string())
                })
                .unwrap();
        }
    });
    barrier.wait();
    for _ in 0..100 {
        cache
            .read_transaction(|db| {
                let values = db.iter()?.collect::<Vec<_>>();
                assert_eq!(values.len(), 2);
                assert_eq!(values[0], values[1]);
                Ok(())
            })
            .unwrap();
        std::thread::yield_now();
    }
    thread.join().unwrap();
}

#[test]
fn concurrent_read_transactions_can_overlap() {
    let mut cache = TypedCache::<String, String>::new();
    cache
        .transaction(|db| db.put(&"key".into(), &"value".into()))
        .unwrap();
    let (started, waiting) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let reader = cache.clone();
    let first = std::thread::spawn(move || {
        reader.read_transaction(|db| {
            started.send(())?;
            released.recv_timeout(std::time::Duration::from_secs(10))?;
            Ok(db.iter()?.collect::<Vec<_>>())
        })
    });
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (completed, completion) = std::sync::mpsc::channel();
    let second = std::thread::spawn(move || {
        let values = cache.read_transaction(|db| Ok(db.iter()?.collect::<Vec<_>>()));
        completed.send(values).unwrap();
    });
    let values = completion.recv_timeout(std::time::Duration::from_secs(5));
    release.send(()).unwrap();
    let first_values = first.join().unwrap().unwrap();
    second.join().unwrap();
    assert_eq!(values.unwrap().unwrap(), ["value"]);
    assert_eq!(first_values, ["value"]);
}
