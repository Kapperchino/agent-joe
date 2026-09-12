use super::*;
use crate::tool_defs::erased_tool;
use analysis::contexts::rust_empty_context::RustEmptyContext;

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
