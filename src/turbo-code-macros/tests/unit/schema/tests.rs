use super::*;

#[test]
fn invalid_shapes_and_constraints_are_rejected() {
    for source in [
        "struct Input(String);",
        "enum Input { Named { field: String } }",
        "#[serde(transparent)] struct Input { first: String, second: String }",
        "#[serde(tag = \"kind\")] enum Input { Pair(String, String) }",
        "#[serde(tag = \"kind\")] enum Input { Named { kind: String } }",
        "struct Input { #[serde(rename = \"same\")] first: String, #[serde(rename = \"same\")] second: String }",
        "enum Input { First, #[serde(rename = \"First\")] Second }",
        "struct Input { #[tool(required, optional)] field: String }",
        "struct Input { #[tool(max_items = \"many\")] field: Vec<String> }",
        "struct Input { #[tool(min_items = 3, max_items = 2)] field: Vec<String> }",
        "struct Input { #[tool(items(min_length = 4, max_length = 2))] field: Vec<String> }",
        "struct Input { #[tool(items(max_length = -1))] field: Vec<String> }",
        "struct Input { #[tool(items(unknown = 2))] field: Vec<String> }",
        "struct Input { #[tool(schema = \"not a type!\")] field: String }",
    ] {
        let input = syn::parse_str::<DeriveInput>(source).unwrap();
        assert!(expand(&input).is_err(), "{source}");
    }
}

#[test]
fn schemas_expand_for_transparent_generic_collection_and_tagged_inputs() {
    for source in [
        "#[serde(transparent)] struct Input<T> { value: T }",
        "#[serde(default)] struct Input { #[tool(items(min_length = 1), max_items = 16)] names: Vec<String> }",
        "struct Input { #[tool(maximum = Limits::MAX)] limit: Option<usize> }",
        "#[serde(tag = \"operation\", rename_all = \"snake_case\")] enum Input { Check(Options), Stop { process_id: String }, Status }",
        "#[serde(rename_all = \"snake_case\")] enum Input { FirstMode, #[serde(rename = \"custom\")] SecondMode }",
    ] {
        let input = syn::parse_str::<DeriveInput>(source).unwrap();
        syn::parse2::<syn::File>(expand(&input).unwrap()).unwrap();
    }
}
