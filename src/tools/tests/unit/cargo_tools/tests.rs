use super::*;
use crate::tool_defs::erased_tool;
use analysis::contexts::rust_empty_context::RustEmptyContext;
use serde_json::json;

#[test]
fn validation_requirements_accept_only_finite_read_only_or_run_commands() {
    for input in [
        json!({"operation":"check"}),
        json!({"operation":"test","package":"selected","test_name":"regression"}),
        json!({"operation":"fmt_check"}),
        json!({"operation":"clippy","deny_warnings":true}),
        json!({"operation":"run","target":{"kind":"example","name":"fixture"}}),
    ] {
        let request: CargoRequest = serde_json::from_value(input).unwrap();
        assert!(request.validation_command().is_ok());
    }
    for input in [
        json!({"operation":"fmt"}),
        json!({"operation":"start","target":{"kind":"example","name":"fixture"}}),
        json!({"operation":"poll","process_id":"id"}),
        json!({"operation":"stop","process_id":"id"}),
        json!({"operation":"check","test_name":"regression"}),
    ] {
        let request: CargoRequest = serde_json::from_value(input).unwrap();
        assert!(request.validation_command().is_err());
    }
}

#[test]
fn cargo_execution_budgets_follow_validated_timeouts() {
    let cargo = erased_tool::<Cargo, RustEmptyContext, ()>();
    for operation in ["check", "test", "fmt", "fmt_check", "clippy"] {
        assert_eq!(
            cargo
                .execution_budget_erased(&json!({"operation":operation}))
                .unwrap(),
            Duration::from_secs(1800)
        );
        assert_eq!(
            cargo
                .execution_budget_erased(&json!({"operation":operation,"timeout_seconds":3600}))
                .unwrap(),
            Duration::from_secs(3600)
        );
        assert!(
            cargo
                .execution_budget_erased(&json!({"operation":operation,"timeout_seconds":3601}))
                .is_err()
        );
    }
    for input in [
        json!({"operation":"start","target":{"kind":"example","name":"server"}}),
        json!({"operation":"poll","process_id":"id"}),
        json!({"operation":"stop","process_id":"id"}),
    ] {
        assert_eq!(
            cargo.execution_budget_erased(&input).unwrap(),
            Duration::ZERO
        );
    }
    let properties = <Cargo as ToolDefTrait>::field_properties();
    let timeout = properties["timeout_seconds"].clone().into_schema();
    assert_eq!(timeout["maximum"], CargoOperation::MAX_TIMEOUT_SECONDS);
    assert_eq!(timeout["default"], CargoOperation::DEFAULT_TIMEOUT_SECONDS);
}

#[test]
fn preparation_rejects_missing_operations_and_incompatible_inputs() {
    let cargo = erased_tool::<Cargo, RustEmptyContext, ()>();
    for input in [
        json!({}),
        json!({"operation":"shell"}),
        json!({"operation":"check","command":"shell"}),
        json!({"operation":"check","package":"--offline=false"}),
        json!({"operation":"check","package":"member","workspace":true}),
        json!({"operation":"check","test_name":"filter"}),
        json!({"operation":"check","target":{"kind":"bin","name":"--config=bad"}}),
        json!({"operation":"check","target":{"kind":"unknown"}}),
        json!({"operation":"check","features":"all"}),
        json!({"operation":"fmt","features":["gated"]}),
        json!({"operation":"start"}),
        json!({"operation":"run","target":{"kind":"lib"}}),
        json!({"operation":"poll"}),
        json!({"operation":"stop","process_id":""}),
        json!({"operation":"poll","process_id":"id","package":"member"}),
        json!({"operation":"stop","process_id":"id","offsets":{"stdout":-1}}),
        json!({"operation":"check","process_id":"id"}),
    ] {
        assert!(cargo.display_erased(&input).is_err(), "{input}");
    }
    assert!(cargo.display_erased(&json!({"operation":"check"})).is_ok());
    assert!(
        cargo
            .display_erased(&json!({"operation":"test","test_name":"regression","exact":true}))
            .is_ok()
    );
    assert!(
        cargo
            .display_erased(
                &json!({"operation":"start","target":{"kind":"example","name":"server"}})
            )
            .is_ok()
    );
    assert!(
        cargo
            .display_erased(&json!({"operation":"poll","process_id":"id","offsets":{"stdout":2}}))
            .is_ok()
    );
}

#[test]
fn worker_permissions_reject_operations_outside_the_advertised_set() {
    let validation = erased_tool::<Cargo<ValidationOperations>, RustEmptyContext, ()>();
    let formatting = erased_tool::<Cargo<FormattingOperations>, RustEmptyContext, ()>();
    assert!(
        validation
            .display_erased(&json!({"operation":"fmt"}))
            .is_err()
    );
    assert!(
        validation
            .display_erased(&json!({"operation":"fmt_check"}))
            .is_ok()
    );
    assert!(
        formatting
            .display_erased(&json!({"operation":"fmt"}))
            .is_ok()
    );
    assert!(
        formatting
            .display_erased(&json!({"operation":"check"}))
            .is_err()
    );
    assert!(
        formatting
            .display_erased(&json!({"operation":"run","target":{"kind":"example","name":"server"}}))
            .is_err()
    );
}

#[test]
fn derived_cargo_schema_preserves_constraints_and_policy_filters() {
    let properties = Cargo::<AllOperations>::field_properties();
    let schema: serde_json::Map<_, _> = properties
        .into_iter()
        .map(|(name, property)| (name, property.into_schema()))
        .collect();
    assert_eq!(
        schema["operation"]["enum"],
        json!(AllOperations::OPERATIONS)
    );
    assert_eq!(Cargo::<AllOperations>::required_fields(), ["operation"]);
    assert_eq!(schema["features"]["maxItems"], 64);
    assert_eq!(schema["features"]["items"]["minLength"], 1);
    assert_eq!(schema["features"]["items"]["maxLength"], 256);
    assert_eq!(schema["args"]["items"]["maxLength"], 4096);
    assert_eq!(schema["environment"]["maxProperties"], 16);
    assert_eq!(
        schema["environment"]["additionalProperties"]["type"],
        "string"
    );
    assert_eq!(
        schema["environment"]["additionalProperties"]["maxLength"],
        4096
    );
    assert_eq!(schema["process_id"]["minLength"], 1);
    assert_eq!(schema["offsets"]["additionalProperties"], false);
    assert_eq!(schema["offsets"]["properties"]["stdout"]["minimum"], 0);
    for kind in ["bin", "example", "test"] {
        let target = schema["target"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["kind"]["enum"] == json!([kind]))
            .unwrap();
        assert_eq!(target["required"], json!(["kind", "name"]));
        assert_eq!(target["properties"]["name"]["minLength"], 1);
        assert_eq!(target["properties"]["name"]["maxLength"], 256);
        assert_eq!(target["additionalProperties"], false);
    }
    let validation = Cargo::<ValidationOperations>::field_properties();
    assert_eq!(
        validation["operation"].clone().into_schema()["enum"],
        json!(ValidationOperations::OPERATIONS)
    );
    let formatting = Cargo::<FormattingOperations>::field_properties();
    assert_eq!(
        formatting["operation"].clone().into_schema()["enum"],
        json!(["fmt"])
    );
    assert_eq!(formatting.len(), FormattingOperations::FIELDS.len());
    assert!(
        FormattingOperations::FIELDS
            .iter()
            .all(|name| formatting.contains_key(*name))
    );
    assert_eq!(
        Cargo::<FormattingOperations>::required_fields(),
        ["operation"]
    );
}
