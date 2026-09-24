use crate::tool_schema::{ToolSchema, input_schema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use turbo_code_macros::ToolSchema;

#[derive(Serialize, Deserialize, ToolSchema)]
#[serde(rename_all = "snake_case")]
enum Mode {
    FirstMode,
    #[serde(rename = "custom")]
    SecondMode,
}

#[derive(Default, Serialize, Deserialize, ToolSchema)]
#[serde(default, deny_unknown_fields)]
struct Options {
    #[serde(rename = "selected")]
    #[tool(required)]
    mode: Option<Mode>,
    #[tool(max_items = 4, items(min_length = 1, max_length = 16))]
    names: Vec<String>,
    #[tool(max_properties = 3, additional_properties(max_length = 32))]
    environment: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, ToolSchema)]
#[serde(transparent)]
struct Wrapper<T> {
    value: T,
}

#[derive(Serialize, Deserialize, ToolSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Configure(Options),
    #[serde(rename = "inspect-tree")]
    InspectTree {
        path: String,
    },
    Status,
}

#[test]
fn derived_schemas_follow_types_defaults_renames_and_collection_constraints() {
    let schema = Wrapper::<Options>::schema();
    assert_eq!(schema["required"], json!(["selected"]));
    assert_eq!(schema["additionalProperties"], false);
    let properties = &schema["properties"];
    assert_eq!(properties["selected"]["type"], json!(["string", "null"]));
    assert_eq!(
        properties["selected"]["enum"],
        json!(["first_mode", "custom", null])
    );
    assert_eq!(properties["names"]["type"], "array");
    assert_eq!(properties["names"]["maxItems"], 4);
    assert_eq!(
        properties["names"]["items"],
        json!({"type":"string", "minLength":1, "maxLength":16})
    );
    assert_eq!(properties["environment"]["maxProperties"], 3);
    assert_eq!(
        properties["environment"]["additionalProperties"],
        json!({"type":"string", "maxLength":32})
    );
    assert_eq!(properties["selected"].as_object().unwrap().len(), 2);
}

#[test]
fn tagged_variants_keep_nested_requirements_and_filter_tool_properties() {
    let schema = Request::schema();
    assert_eq!(
        schema["oneOf"][0]["required"],
        json!(["action", "selected"])
    );
    assert_eq!(schema["oneOf"][1]["required"], json!(["action", "path"]));
    let input = input_schema(schema.clone(), &[], &[]);
    assert_eq!(input["required"], json!(["action"]));
    assert_eq!(
        input["properties"]["action"]["enum"],
        json!(["configure", "inspect-tree", "status"])
    );
    let selected = input_schema(schema, &["inspect-tree"], &["action", "path"]);
    assert_eq!(selected["required"], json!(["action", "path"]));
    assert_eq!(
        selected["properties"]["action"]["enum"],
        json!(["inspect-tree"])
    );
    assert_eq!(selected["properties"].as_object().unwrap().len(), 2);
}

#[derive(Serialize, Deserialize, ToolSchema)]
#[serde(tag = "action")]
enum Mixed {
    Text { value: String },
    Count { value: usize },
}

#[test]
fn variant_field_types_are_preserved_in_the_flattened_tool_schema() {
    let schema = input_schema(Mixed::schema(), &[], &[]);
    assert_eq!(
        schema["properties"]["value"],
        json!({"anyOf": [{"type":"string"}, {"type":"integer"}]})
    );
    assert_eq!(
        Option::<Option<Mixed>>::schema()["oneOf"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
