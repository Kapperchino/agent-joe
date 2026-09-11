use super::*;

#[test]
fn unsupported_shapes_and_invalid_attributes_report_errors() {
    for source in [
        "struct Input(String);",
        "enum Input { Status }",
        "#[serde(tag = \"operation\")] enum Input { Show(String) }",
        "#[serde(tag = \"operation\", content = \"input\")] enum Input { Status }",
        "struct Input { #[tool(minimum = \"one\")] limit: usize }",
        "struct Input { #[tool(minimum = 10, maximum = 1)] limit: usize }",
        "struct Input { #[tool(minimum = 1)] path: String }",
        "struct Input { #[tool(values(\"one\"))] limit: usize }",
        "struct Input { #[tool(values())] path: String }",
        "struct Input { #[tool(kind = \"file\")] path: String }",
        "struct Input { #[tool(unknown)] path: String }",
        "struct Input { #[tool(required = true)] path: String }",
        "struct Input { #[tool(description = \"first\")] #[tool(description = \"second\")] path: String }",
        "#[serde(tag = \"operation\")] enum Input { Show { operation: String } }",
        "#[serde(tag = \"operation\")] enum Input { First { value: String }, Second { value: usize } }",
        "#[serde(tag = \"operation\")] enum Input { First, #[serde(rename = \"First\")] Second }",
    ] {
        let input = syn::parse_str::<DeriveInput>(source).unwrap();
        assert!(InputSchema::new(&input).is_err(), "{source}");
    }
}

#[test]
fn malformed_bounds_are_not_silently_dropped() {
    let input = syn::parse_quote! {
        struct Input {
            #[tool(description = "Maximum commits")]
            #[tool(minimum = "one", maximum = 100)]
            limit: Option<usize>,
        }
    };
    let error = InputSchema::new(&input).err().unwrap();
    assert!(error.to_string().contains("nonnegative integer"));
}
