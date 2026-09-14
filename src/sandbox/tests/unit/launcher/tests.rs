use super::*;

#[test]
fn host_resources_use_all_cpus_and_half_memory_across_system_sizes() {
    for cpu_count in [1, 2, 12, 64, 255] {
        for memory_mib in [2, 3, 512, 8192, 32768, 131072] {
            for page_size in [4096, 16384, 65536] {
                let physical_pages = memory_mib * 1024 * 1024 / page_size;
                let resources = HostResources::new(cpu_count, physical_pages, page_size).unwrap();
                assert_eq!(libc::c_long::from(resources.vcpus), cpu_count);
                assert_eq!(libc::c_long::from(resources.memory_mib), memory_mib / 2);
            }
        }
    }
}

#[test]
fn half_host_memory_is_rounded_down_to_whole_mib() {
    for extra_pages in [0, 1, 256, 511] {
        let resources =
            HostResources::new(12, 32 * 1024 * 1024 * 1024 / 4096 + extra_pages, 4096).unwrap();
        assert_eq!(resources.memory_mib, 16384);
    }
}

#[test]
fn host_resources_reject_missing_or_zero_system_values() {
    for invalid in [-1, 0] {
        assert!(HostResources::new(invalid, 1024 * 1024, 4096).is_err());
        assert!(HostResources::new(12, invalid, 4096).is_err());
        assert!(HostResources::new(12, 1024 * 1024, invalid).is_err());
    }
    assert!(HostResources::new(12, 1, 4096).is_err());
}

#[test]
fn host_resources_require_at_least_one_mib_after_halving() {
    for physical_pages in [256, 511] {
        assert!(HostResources::new(12, physical_pages, 4096).is_err());
    }
    let resources = HostResources::new(12, 512, 4096).unwrap();
    assert_eq!(resources.memory_mib, 1);
}

#[test]
fn host_resources_reject_overflow_instead_of_truncating_capacity() {
    assert!(HostResources::new(256, 1024 * 1024, 4096).is_err());
    let oversized_pages = (libc::c_long::from(u32::MAX) + 1) * 512;
    assert!(HostResources::new(12, oversized_pages, 4096).is_err());
    assert!(HostResources::new(12, libc::c_long::MAX, libc::c_long::MAX).is_err());
}

#[test]
fn host_resources_preserve_the_largest_representable_capacity() {
    let resources = HostResources::new(255, libc::c_long::from(u32::MAX) * 512, 4096).unwrap();
    assert_eq!(resources.vcpus, u8::MAX);
    assert_eq!(resources.memory_mib, u32::MAX);
}

#[test]
fn detected_host_resources_use_half_system_memory_and_are_accepted_by_libkrun() {
    let resources = HostResources::detect().unwrap();
    let cpu_count = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    let physical_pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    assert_eq!(libc::c_long::from(resources.vcpus), cpu_count);
    assert_eq!(
        libc::c_long::from(resources.memory_mib),
        physical_pages * page_size / 2 / (1024 * 1024)
    );
    let context = KrunContext::new(Box::default()).unwrap();
    assert_eq!(
        krun::krun_set_vm_config(context.id, resources.vcpus, resources.memory_mib),
        0
    );
}

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
