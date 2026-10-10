use super::*;

#[test]
fn completion_freezes_status_and_retained_output() {
    let process = ProcessHandle::new(
        ProcessCommand {
            program: "cargo".into(),
            args: vec!["check".into()],
            environment: FnvHashMap::default(),
        },
        CancellationToken::new(),
    );
    assert!(process.append(OutputStream::Stdout, b"ready", 64));
    process.complete(ProcessEnd::Exited, Some(0));
    let completed = process.output();
    assert!(completed.success());
    assert_eq!(completed.stdout, "ready");
    assert!(!process.append(OutputStream::Stdout, b"late", 64));
    process.complete(ProcessEnd::Cancelled, None);
    assert_eq!(
        serde_json::to_value(process.output()).unwrap(),
        serde_json::to_value(completed).unwrap()
    );
}

#[test]
fn incremental_text_retains_a_stable_prefix_across_split_utf8() {
    let incomplete = b"\xffready \xe9\x9b";
    assert_eq!(output_text(incomplete, &ProcessStatus::Running), "�ready ");
    assert_eq!(
        output_text(b"\xffready \xe9\x9b\xaa", &ProcessStatus::Running),
        "�ready 雪"
    );
    assert_eq!(
        output_text(incomplete, &ProcessStatus::Cancelled),
        "�ready �"
    );
}

#[test]
fn concurrent_registration_preserves_the_eight_target_limit() {
    let registry = Arc::new(ProcessRegistry::default());
    let barrier = Arc::new(std::sync::Barrier::new(32));
    let threads = (0..32)
        .map(|_| {
            let registry = registry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let process = ProcessHandle::new(
                    ProcessCommand {
                        program: "cargo".into(),
                        args: vec!["check".into()],
                        environment: FnvHashMap::default(),
                    },
                    CancellationToken::new(),
                );
                barrier.wait();
                registry.insert(process.clone()).map(|id| {
                    assert!(Arc::ptr_eq(&registry.get(&id).unwrap(), &process));
                    process.complete(ProcessEnd::Exited, Some(0));
                    assert!(registry.get(&id).unwrap().output().success());
                    id
                })
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 8);
    assert_eq!(registry.entries.len(), 8);
    for error in results.into_iter().filter_map(Result::err) {
        assert_eq!(
            error.to_string(),
            "The turn has reached its limit of eight managed targets"
        );
    }
    assert!(registry.get("unknown").is_err());
}
