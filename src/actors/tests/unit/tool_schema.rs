use crate::tools::{
    knowledge::Knowledge, request_user_input::RequestUserInput, update_plan::UpdatePlan,
};
use serde_json::{Value, json};
use tools::tool_defs::{LenientDeserialize, ToolDefTrait};

fn properties<T: ToolDefTrait>() -> Value {
    Value::Object(
        T::field_properties()
            .into_iter()
            .map(|(name, property)| (name, property.into_schema()))
            .collect(),
    )
}

#[test]
fn knowledge_schema_is_derived_from_actions_and_typed_options() {
    let schema = properties::<Knowledge>();
    assert_eq!(Knowledge::required_fields(), ["action"]);
    assert_eq!(
        schema["action"]["enum"],
        json!([
            "prepare",
            "repartition",
            "status",
            "search",
            "inspect",
            "clear"
        ])
    );
    assert_eq!(schema["features"]["type"], json!(["array", "null"]));
    assert_eq!(schema["features"]["maxItems"], 128);
    assert_eq!(
        schema["default_features"]["enum"],
        json!(["enabled", "disabled", null])
    );
    assert_eq!(
        schema["configurations"]["items"]["enum"],
        json!(["normal", "test"])
    );
    assert_eq!(schema["configurations"]["minItems"], 1);
    assert_eq!(schema["configurations"]["maxItems"], 2);
    assert_eq!(schema["limit"]["minimum"], 1);
    assert_eq!(schema["limit"]["maximum"], 32);
    assert_eq!(schema["offset"]["minimum"], 0);
    for name in ["generation", "query", "symbol"] {
        assert_eq!(schema[name]["type"], json!(["string", "null"]));
    }
    assert!(
        crate::tools::knowledge::Input::deserialize_lenient(
            json!({"action":"status","offset":null,"limit":null})
        )
        .is_ok()
    );
}

#[test]
fn question_schema_keeps_required_fields_and_hides_runtime_purpose() {
    let schema = properties::<RequestUserInput>();
    assert_eq!(
        RequestUserInput::required_fields(),
        ["id", "prompt", "required"]
    );
    assert!(schema.get("purpose").is_none());
    assert_eq!(schema["choices"]["maxItems"], 6);
    assert_eq!(
        schema["choices"]["items"]["required"],
        json!(["id", "label"])
    );
    assert_eq!(schema["choices"]["items"]["additionalProperties"], false);
    assert!(
        crate::tools::request_user_input::Input::deserialize_lenient(
            json!({"id":"target","prompt":"Which target?","required":true})
        )
        .is_ok()
    );
    assert!(
        crate::tools::request_user_input::Input::deserialize_lenient(
            json!({"id":"../bad","prompt":"Which target?","required":true})
        )
        .is_err()
    );
}

#[test]
fn plan_schema_includes_typed_steps_evidence_and_cargo_validation() {
    let schema = properties::<UpdatePlan>();
    assert_eq!(
        UpdatePlan::required_fields(),
        ["revision", "requirements_revision", "steps"]
    );
    assert_eq!(schema["steps"]["minItems"], 1);
    assert_eq!(schema["steps"]["maxItems"], 16);
    let step = &schema["steps"]["items"];
    assert_eq!(
        step["required"],
        json!([
            "id",
            "description",
            "dependencies",
            "acceptance",
            "state",
            "evidence",
            "blocked_reason"
        ])
    );
    assert_eq!(step["additionalProperties"], false);
    assert_eq!(
        step["properties"]["state"]["enum"],
        json!(["pending", "in_progress", "completed", "blocked"])
    );
    assert_eq!(
        step["properties"]["kind"]["enum"],
        json!(["investigation", "implementation"])
    );
    assert_eq!(
        step["properties"]["evidence"]["items"]["required"],
        json!(["source", "explanation"])
    );
    let validation = &step["properties"]["validation"]["oneOf"];
    assert_eq!(
        validation[0]["properties"]["operation"]["enum"],
        json!(["check"])
    );
    assert_eq!(validation[0]["properties"]["features"]["maxItems"], 64);
    assert!(
        validation
            .as_array()
            .unwrap()
            .contains(&json!({"type":"null"}))
    );
}

#[test]
fn generated_actor_properties_survive_provider_serialization() {
    for properties in [
        Knowledge::field_properties(),
        RequestUserInput::field_properties(),
        UpdatePlan::field_properties(),
    ] {
        for property in properties.into_values() {
            let expected = property.clone().into_schema();
            let openai: clients::openai::ToolProperty = property.clone().into();
            let claude: clients::claude::ToolProperty = property.into();
            assert_eq!(serde_json::to_value(openai).unwrap(), expected);
            assert_eq!(serde_json::to_value(claude).unwrap(), expected);
        }
    }
}
