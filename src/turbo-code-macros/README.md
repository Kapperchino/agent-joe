# Tool schema derives

Tool definitions use `ToolDef`; do not handwrite `field_properties` or maintain
parallel JSON property maps.

For tools that store their input, derive `ToolInput` on the input and mark the
stored field with `#[tool(input)]`. This generates the schema, request fields,
and lenient deserialization through the existing `tools` interfaces.

For stateless tools, specify the input type on the definition:

```rust
#[derive(ToolDef)]
#[tool(name = "inspect", description = "Inspect the project", input = "Input")]
pub struct Inspect;
```

Derive `ToolSchema` on that input and its nested domain types. The schema-only
derive implements `common_models::tool_schema::ToolSchema` and does not generate
deserialization or depend on the higher-level `tools` crate. This lets validated
types retain their existing Serde implementations.

`ToolSchema` supports named structs, transparent wrappers, string enums,
internally tagged enums with named or newtype payloads, and generic nested types.
Primitive types, `Option`, `Vec`, and string-keyed maps have shared implementations.
Serde field renames, defaults, enum renames, and `deny_unknown_fields` are reflected
in the schema. Unsupported Serde attributes are rejected rather than ignored.

Field metadata belongs beside its Rust field:

```rust
#[tool(description = "Feature names", max_items = 64,
       items(min_length = 1, max_length = 256))]
features: Vec<String>,
```

Available constraints include `minimum`, `maximum`, `min_length`, `max_length`,
`min_items`, `max_items`, `max_properties`, and `default`. Bounds accept nonnegative
integer literals or constant paths. Use `items(...)` for array elements and
`additional_properties(...)` for map values. `required`, `optional`, `nullable`,
and `skip` control the advertised contract. `schema = "Type"` selects a schema
type for a validated wrapper or a generic schema specialization without changing
its runtime deserialization.

Stateless `ToolDef` also accepts `variants = "P::OPERATIONS"` and
`fields = "P::FIELDS"` for worker-specific restrictions. These expressions are
string slices; an empty slice applies no filter. Tagged enum inputs are flattened
into the tool's property map while nested tagged values retain their variant
schemas. Runtime permission checks remain responsible for enforcing worker
restrictions.