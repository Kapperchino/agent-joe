use crate::{
    git::{Git, GitInput},
    read_file::{ReadFile, ReadFileInput},
    tool_defs::{LenientDeserialize, NonEmptyString, ToolDefTrait, ToolInputSchema},
    worktree::Worktree,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use turbo_code_macros::ToolInput;

#[derive(Serialize, Deserialize, ToolInput)]
#[serde(tag = "action", rename_all = "snake_case")]
enum TaggedInput {
    #[serde(rename = "inspect-tree")]
    InspectTree {
        #[serde(rename = "path")]
        #[tool(kind = "string", description = "Literal path")]
        location: NonEmptyString,
    },
    ListAll,
}

#[derive(Serialize, Deserialize, ToolInput)]
#[serde(deny_unknown_fields)]
struct DefaultedInput {
    #[serde(rename = "query")]
    #[tool(description = "Search text", required)]
    text: String,
    #[serde(default)]
    #[tool(description = "Include hidden files")]
    hidden: bool,
    #[tool(description = "Maximum results")]
    #[tool(minimum = 1, maximum = 100)]
    limit: Option<usize>,
}

#[derive(Serialize, Deserialize, ToolInput)]
struct NestedInput<T: ToolInputSchema> {
    #[tool(description = "Nested input")]
    value: Option<T>,
}

#[test]
fn git_and_worktree_schemas_advertise_nullable_options_and_constraints() {
    let git = Git::field_properties();
    let operation = git["operation"].clone().into_schema();
    assert_eq!(operation["enum"], json!(["status", "diff", "show", "log"]));
    assert_eq!(Git::required_fields(), vec!["operation"]);
    for name in ["path", "revision", "target"] {
        assert_eq!(
            git[name].clone().into_schema()["type"],
            json!(["string", "null"])
        );
    }
    assert_eq!(
        git["target"].clone().into_schema()["enum"],
        json!(["staged", "unstaged", "head", null])
    );
    let limit = git["limit"].clone().into_schema();
    assert_eq!(limit["type"], json!(["integer", "null"]));
    assert_eq!(limit["minimum"], json!(1));
    assert_eq!(limit["maximum"], json!(100));
    let worktree = Worktree::field_properties();
    for name in ["id", "base", "dirty_source"] {
        assert_eq!(
            worktree[name].clone().into_schema()["type"],
            json!(["string", "null"])
        );
    }
    assert_eq!(
        worktree["dirty_source"].clone().into_schema()["enum"],
        json!(["reject", "base_only", null])
    );
}

#[test]
fn tagged_enum_names_and_requests_follow_serde() {
    let properties = TaggedInput::properties();
    assert_eq!(
        properties["action"].clone().into_schema()["enum"],
        json!(["inspect-tree", "list_all"])
    );
    assert_eq!(TaggedInput::required(), vec!["action"]);
    assert!(properties.contains_key("path"));
    assert!(!properties.contains_key("location"));
    let value = json!({"action":"inspect-tree","path":"src/lib.rs"});
    let input = TaggedInput::deserialize_lenient(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&input).unwrap(), value);
    let request = input.req().unwrap();
    assert_eq!(request["action"], "inspect-tree");
    assert_eq!(request["path"], "src/lib.rs");
    let input = TaggedInput::deserialize_lenient(json!({"action":"list_all","path":null})).unwrap();
    assert_eq!(input.req().unwrap().len(), 1);
    for value in [
        json!({"action":"inspect-tree"}),
        json!({"action":"inspect-tree","path":null}),
        json!({"action":"inspect-tree","path":""}),
        json!({"action":"InspectTree","path":"src/lib.rs"}),
        json!({}),
        json!([]),
    ] {
        assert!(
            TaggedInput::deserialize_lenient(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn derived_requests_include_only_fields_for_the_selected_operation() {
    let input = GitInput::deserialize_lenient(json!({
        "operation":"diff","path":"src/lib.rs","target":"unstaged","revision":"HEAD","limit":20
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(input.req().unwrap()).unwrap(),
        json!({
            "operation":"diff","path":"src/lib.rs","target":"unstaged"
        })
    );
    let input = GitInput::deserialize_lenient(json!({"operation":"log","limit":3})).unwrap();
    assert_eq!(input.req().unwrap()["limit"], "3");
}

#[test]
fn existing_struct_inputs_keep_nested_fields_and_requests() {
    let properties = ReadFile::field_properties();
    let range = properties["range"].clone().into_schema();
    assert_eq!(range["type"], json!(["object", "null"]));
    assert_eq!(range["required"], json!(["start", "end"]));
    assert_eq!(range["properties"]["start"]["type"], json!("integer"));
    let input = ReadFileInput::deserialize_lenient(json!({
        "file_path":"src/lib.rs","range":{"start":2,"end":5}
    }))
    .unwrap();
    let request = input.req().unwrap();
    assert_eq!(request["file_path"], "src/lib.rs");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&request["range"]).unwrap(),
        json!({"start":2,"end":5})
    );
    let input = ReadFileInput::deserialize_lenient(json!({"file_path":"src/lib.rs"})).unwrap();
    assert!(!input.req().unwrap().contains_key("range"));
}

#[test]
fn struct_defaults_and_unknown_field_rules_follow_serde() {
    assert_eq!(DefaultedInput::required(), vec!["query"]);
    let input = DefaultedInput::deserialize_lenient(json!({"query":"needle"})).unwrap();
    assert!(!input.hidden);
    assert!(input.limit.is_none());
    assert_eq!(input.req().unwrap()["query"], "needle");
    assert!(
        DefaultedInput::deserialize_lenient(json!({"query":"needle","unexpected":true})).is_err()
    );
    assert!(DefaultedInput::deserialize_lenient(json!({"query":"needle","limit":"many"})).is_err());
    let nested =
        NestedInput::<DefaultedInput>::deserialize_lenient(json!({"value":{"query":"needle"}}))
            .unwrap();
    assert!(nested.value.is_some());
    let schema = NestedInput::<DefaultedInput>::properties()["value"]
        .clone()
        .into_schema();
    assert_eq!(schema["properties"]["limit"]["maximum"], json!(100));
}
