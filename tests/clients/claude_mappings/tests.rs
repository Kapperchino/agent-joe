use super::*;
use serde_json::json;

#[test]
fn maps_search_tool_definition_to_claude_server_tool() {
    let definition = ToolDefinition::Search {
        name: "web_search".to_string(),
    };

    let tool: Tool = (&definition).into();
    let value = serde_json::to_value(tool).unwrap();

    assert_eq!(
        value,
        json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "max_uses": 5
        })
    );
}
