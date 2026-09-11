use super::*;
use crate::tool_defs::erased_tool;
use analysis::contexts::rust_empty_context::RustEmptyContext;

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
