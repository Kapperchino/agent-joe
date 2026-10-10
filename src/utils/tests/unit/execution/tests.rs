use super::*;

#[test]
fn concurrent_registrations_are_shared_sorted_and_removed_on_drop() {
    let scope = ExecutionScope::default();
    let child = scope.child();
    let threads = (0..32)
        .map(|index| {
            let scope = child.clone();
            std::thread::spawn(move || scope.register(ResourceKind::Tool, index.to_string()))
        })
        .collect::<Vec<_>>();
    let mut registrations = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    let resources = scope.resources();
    assert_eq!(resources.len(), 32);
    assert!(resources.windows(2).all(|pair| pair[0].id < pair[1].id));
    assert!(
        resources
            .iter()
            .all(|resource| resource.kind == ResourceKind::Tool)
    );
    assert_eq!(
        resources
            .iter()
            .map(|resource| resource.id)
            .collect::<Vec<_>>(),
        child
            .resources()
            .iter()
            .map(|resource| resource.id)
            .collect::<Vec<_>>()
    );
    drop(registrations.split_off(16));
    assert_eq!(scope.resources().len(), 16);
    assert_eq!(child.resources().len(), 16);
    drop(registrations);
    assert!(scope.resources().is_empty());
    assert!(child.resources().is_empty());
}
