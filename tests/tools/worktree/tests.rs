use super::*;
use crate::tool_defs::LenientDeserialize;
use serde_json::{Value, json};

#[test]
fn unused_fields_do_not_prevent_listing_worktrees() {
    let input = WorktreeInput::deserialize_lenient(json!({
        "operation":"list","id":"","base":"","dirty_source":"reject"
    }))
    .unwrap();
    assert!(matches!(
        WorktreeOperation::try_from(input).unwrap(),
        WorktreeOperation::List
    ));
}

#[test]
fn creation_requires_a_base_and_explicit_opt_in_to_dirty_sources() {
    for dirty_source in [Value::Null, json!(""), json!("reject")] {
        let input = WorktreeInput::deserialize_lenient(json!({
            "operation":"create","base":"HEAD","dirty_source":dirty_source
        }))
        .unwrap();
        assert!(matches!(
            WorktreeOperation::try_from(input).unwrap(),
            WorktreeOperation::Create {
                dirty: DirtySource::Reject,
                ..
            }
        ));
    }
    let input = WorktreeInput::deserialize_lenient(json!({
        "operation":"create","base":"HEAD","dirty_source":"base_only"
    }))
    .unwrap();
    assert!(matches!(
        WorktreeOperation::try_from(input).unwrap(),
        WorktreeOperation::Create {
            dirty: DirtySource::BaseOnly,
            ..
        }
    ));
    for value in [
        json!({"operation":"create"}),
        json!({"operation":"create","base":""}),
        json!({"operation":"create","base":"HEAD","dirty_source":"copy"}),
        json!({"operation":"remove"}),
        json!({"operation":"remove","id":""}),
        json!({"operation":"integrate","id":null}),
    ] {
        assert!(
            WorktreeInput::deserialize_lenient(value.clone())
                .and_then(WorktreeOperation::try_from)
                .is_err(),
            "{value}"
        );
    }
}
