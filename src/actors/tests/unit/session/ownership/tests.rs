use super::*;

#[test]
fn ownership_distinguishes_process_incarnations_and_rejects_unsupported_schema() {
    let owner = Owner::new(None, "first".into()).unwrap();
    assert!(Owner::new(Some(owner.clone()), "second".into()).is_err());
    let mut previous = owner.clone();
    previous.process.started = "previous boot or reused PID".into();
    let reclaimed = Owner::new(Some(previous), "second".into()).unwrap();
    assert_eq!(reclaimed.token, "second");
    assert!(reclaimed.process == owner.process);
    let mut previous = serde_json::to_value(owner).unwrap();
    previous["version"] = serde_json::json!(999);
    assert!(serde_json::from_value::<Owner>(previous).is_err());
}
