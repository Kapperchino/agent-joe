use super::*;

#[test]
fn completion_freezes_status_and_retained_output() {
    let process = ProcessHandle::new(
        ProcessCommand {
            program: "cargo".into(),
            args: vec!["check".into()],
            environment: BTreeMap::new(),
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
