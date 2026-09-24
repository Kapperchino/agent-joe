use super::*;

#[test]
fn explicit_input_supports_stateless_and_generic_policy_tools() {
    for source in [
        "#[tool(name = \"inspect\", description = \"Inspect\", input = \"Input\")] struct Inspect;",
        "#[tool(name = \"cargo\", description = \"Cargo\", input = \"Input\", variants = \"P::OPERATIONS\", fields = \"P::FIELDS\")] struct Cargo<P: Policy>(std::marker::PhantomData<P>);",
    ] {
        let input = syn::parse_str::<DeriveInput>(source).unwrap();
        syn::parse2::<syn::File>(tool_definition(&input).unwrap()).unwrap();
    }
}

#[test]
fn ambiguous_inputs_and_misplaced_filters_are_rejected() {
    for source in [
        "#[tool(name = \"inspect\", description = \"Inspect\", input = \"Input\")] enum Inspect { All }",
        "#[tool(name = \"inspect\", description = \"Inspect\", input = \"Input\")] struct Inspect { #[tool(input)] input: Input }",
        "#[tool(name = \"inspect\", description = \"Inspect\", fields = \"FIELDS\")] struct Inspect { #[tool(input)] input: Input }",
        "#[tool(name = \"inspect\", description = \"Inspect\", input = \"not a type!\")] struct Inspect;",
    ] {
        let input = syn::parse_str::<DeriveInput>(source).unwrap();
        assert!(tool_definition(&input).is_err(), "{source}");
    }
}
