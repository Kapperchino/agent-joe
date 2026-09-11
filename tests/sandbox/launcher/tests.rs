use super::*;

#[test]
fn dropping_a_context_releases_it_in_the_crate() {
    let context = KrunContext::new(Box::default()).unwrap();
    let id = context.id;
    assert_eq!(krun::krun_set_vm_config(id, 2, 4096), 0);
    drop(context);
    assert_eq!(krun::krun_set_vm_config(id, 2, 4096), -libc::ENOENT);
}

#[test]
fn crate_errors_retain_the_operation_and_cause() {
    let context = KrunContext::new(Box::default()).unwrap();
    let error = result(
        "krun_set_vm_config",
        krun::krun_set_vm_config(context.id, 0, 4096),
    )
    .unwrap_err();
    assert!(error.to_string().contains("krun_set_vm_config"));
    assert!(
        error
            .to_string()
            .contains(&std::io::Error::from_raw_os_error(libc::EINVAL).to_string())
    );
}
