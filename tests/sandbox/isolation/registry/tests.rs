use super::*;

#[test]
fn cargo_fetch_requests_missing_indexes_and_archives() {
    for diagnostic in [
        "error: no matching package named `itoa` found\nlocation searched: crates.io index",
        "error: failed to select a version for the requirement `itoa = \"=1.0.14\"`",
    ] {
        assert_eq!(
            RegistryRequest::from_diagnostic(diagnostic).unwrap(),
            Some(RegistryRequest::Index {
                index: RegistryIndex::new("itoa".into()).unwrap(),
            })
        );
    }
    assert_eq!(
            RegistryRequest::from_diagnostic(
                "error: failed to download `itoa v1.0.14`\n\nCaused by:\n  attempting to make an HTTP request, but --offline was specified"
            )
            .unwrap(),
            Some(RegistryRequest::Packages)
        );
    for diagnostic in [
        "error: failed to parse manifest at `/workspace/Cargo.toml`",
        "error: failed to download `itoa v1.0.14`\nCaused by:\n  checksum mismatch",
        "error: failed to get `demo` as a dependency\nCaused by:\n  can't checkout from 'https://example.com/demo': you are in the offline mode (--offline)",
    ] {
        assert_eq!(RegistryRequest::from_diagnostic(diagnostic).unwrap(), None);
    }
}

#[test]
fn locked_dependencies_cannot_redirect_downloads_or_cache_writes() {
    for name in ["../escape", "x/y", "", "https://host"] {
        assert!(
            RegistryPackage::new(LockedPackage {
                name: name.into(),
                version: "1.0.0".into(),
                checksum: Some("a".repeat(64)),
                source: None,
            })
            .is_err()
        );
    }
}

#[test]
fn sparse_metadata_must_match_the_locked_package_checksum() {
    let package = RegistryPackage::new(LockedPackage {
        name: "demo".into(),
        version: "1.0.0".into(),
        checksum: Some("a".repeat(64)),
        source: None,
    })
    .unwrap();
    let correct = format!(
        r#"{{"name":"demo","vers":"1.0.0","cksum":"{}"}}"#,
        "a".repeat(64)
    );
    assert!(
        SparseIndex::new(&correct, Some(&package))
            .unwrap()
            .bytes
            .split(|byte| *byte == 0)
            .any(|entry| package.matches(entry))
    );
    assert!(SparseIndex::new(&correct.replace('a', "b"), Some(&package)).is_err());
}
