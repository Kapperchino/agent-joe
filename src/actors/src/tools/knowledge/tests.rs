use super::*;

#[test]
fn preparation_defaults_and_explicit_empty_features_keep_the_same_profile() {
    let default = PreparationProfile::new(None, None, None).unwrap().semantic;
    let explicit = PreparationProfile::new(Some(Vec::new()), None, None)
        .unwrap()
        .semantic;
    assert_eq!(default, explicit);
    assert_eq!(default.features, Features::Default);
    assert_eq!(
        default.configurations,
        BTreeSet::from([Configuration::Normal, Configuration::Test])
    );
    assert_eq!(default.manifest.as_str(), "Cargo.toml");
    assert_eq!(default.target, utils::knowledge::native_target());
    assert_eq!(default.analyzer_version, ANALYZER_VERSION);
}

#[test]
fn preparation_normalizes_selection_without_losing_disabled_defaults() {
    let disabled = PreparationProfile::new(None, Some(DefaultFeatures::Disabled), None)
        .unwrap()
        .semantic;
    assert_eq!(disabled.features, Features::None);
    let selected = PreparationProfile::new(
        Some(vec!["app/feature".into(); 129]),
        Some(DefaultFeatures::Disabled),
        Some(vec![Configuration::Test, Configuration::Test]),
    )
    .unwrap()
    .semantic;
    assert_eq!(
        selected.features,
        Features::Named {
            names: BTreeSet::from(["app/feature".into()]),
            defaults: DefaultFeatures::Disabled,
        }
    );
    assert_eq!(
        selected.configurations,
        BTreeSet::from([Configuration::Test])
    );
}

#[test]
fn preparation_rejects_invalid_names_and_empty_configurations() {
    for name in [
        String::new(),
        "two words".into(),
        "a\n".into(),
        "a".repeat(257),
    ] {
        assert!(PreparationProfile::new(Some(vec![name]), None, None).is_err());
    }
    assert!(PreparationProfile::new(None, None, Some(Vec::new())).is_err());
    let names = (0..129).map(|index| format!("feature-{index}")).collect();
    assert!(PreparationProfile::new(Some(names), None, None).is_err());
    let names = (0..128).map(|index| format!("feature-{index}")).collect();
    assert!(PreparationProfile::new(Some(names), None, None).is_ok());
    assert!(PreparationProfile::new(Some(vec!["a".repeat(256)]), None, None).is_ok());
}

#[test]
fn search_request_defaults_and_bounds_preserve_query_and_generation() {
    let default = SearchRequest::new("query".into(), None, None, None).unwrap();
    assert_eq!(default.query, "query");
    assert!(default.generation.is_none());
    assert_eq!(default.offset, 0);
    assert_eq!(default.limit, 10);
    for limit in [1, 32] {
        let request = SearchRequest::new(
            "query".into(),
            Some("generation".into()),
            Some(5),
            Some(limit),
        )
        .unwrap();
        assert_eq!(request.generation.as_deref(), Some("generation"));
        assert_eq!(request.offset, 5);
        assert_eq!(request.limit, limit);
    }
    for limit in [0, 33, usize::MAX] {
        assert!(SearchRequest::new("query".into(), None, None, Some(limit)).is_err());
    }
}
