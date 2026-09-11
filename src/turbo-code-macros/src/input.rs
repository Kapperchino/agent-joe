use crate::attributes::Attributes;
use proc_macro2::TokenStream;
use quote::quote;
use std::collections::BTreeMap;
use syn::{Data, DeriveInput, Field, Fields, GenericArgument, PathArguments, Type, ext::IdentExt};

pub(crate) struct InputSchema {
    fields: Vec<FieldSchema>,
    required: Vec<String>,
}

impl InputSchema {
    pub fn new(input: &DeriveInput) -> syn::Result<Self> {
        match &input.data {
            Data::Struct(data) => {
                Attributes::new(&input.attrs, "serde", &["default", "deny_unknown_fields"])?;
                let fields = named_fields(&data.fields)?
                    .iter()
                    .map(|field| FieldSchema::new(field, false))
                    .collect::<syn::Result<Vec<_>>>()?;
                let required = fields
                    .iter()
                    .filter(|field| field.required)
                    .map(|field| field.name.clone())
                    .collect();
                Ok(Self { fields, required })
            }
            Data::Enum(data) => {
                let serde = Attributes::new(
                    &input.attrs,
                    "serde",
                    &["tag", "rename_all", "deny_unknown_fields"],
                )?;
                let tag = serde.string("tag")?.ok_or_else(|| {
                    syn::Error::new_spanned(
                        input,
                        "ToolInput enums require #[serde(tag = \"...\")]",
                    )
                })?;
                let naming = VariantNaming::new(serde.string("rename_all")?, input)?;
                let options = Attributes::new(&input.attrs, "tool", &["description"])?;
                let description = options.string("description")?.unwrap_or_default();
                let variants = data
                    .variants
                    .iter()
                    .map(|variant| {
                        let serde = Attributes::new(&variant.attrs, "serde", &["rename"])?;
                        let name = serde
                            .string("rename")?
                            .unwrap_or_else(|| naming.apply(&variant.ident.unraw().to_string()));
                        let fields = match &variant.fields {
                            Fields::Unit => Ok(Vec::new()),
                            fields => named_fields(fields)?
                                .iter()
                                .map(|field| FieldSchema::new(field, true))
                                .collect::<syn::Result<Vec<_>>>(),
                        }?;
                        Ok(VariantSchema { name, fields })
                    })
                    .collect::<syn::Result<Vec<_>>>()?;
                let names = variants
                    .iter()
                    .map(|variant| &variant.name)
                    .collect::<Vec<_>>();
                let unique = names.iter().collect::<std::collections::BTreeSet<_>>();
                match !names.is_empty() && names.len() == unique.len() {
                    true => Ok(()),
                    false => Err(syn::Error::new_spanned(
                        input,
                        "ToolInput requires distinct, nonempty enum variants",
                    )),
                }?;
                let tag_schema = FieldSchema {
                    name: tag.clone(),
                    required: true,
                    schema: quote!(serde_json::json!({
                        "type":"string", "enum":[#(#names),*], "description":#description
                    })),
                };
                let fields = variants.into_iter().flat_map(|variant| variant.fields)
                    .try_fold(BTreeMap::<String, FieldSchema>::new(), |mut fields, field| {
                        match fields.get(&field.name) {
                            _ if field.name == tag => Err(syn::Error::new_spanned(input, "Enum field conflicts with the Serde tag")),
                            Some(existing) if existing.schema.to_string() != field.schema.to_string() => {
                                Err(syn::Error::new_spanned(input, format!(
                                    "Shared enum field '{}' must have the same tool schema in every variant", field.name
                                )))
                            }
                            _ => {
                                fields.insert(field.name.clone(), field);
                                Ok(fields)
                            }
                        }
                    })?;
                Ok(Self {
                    fields: std::iter::once(tag_schema)
                        .chain(fields.into_values())
                        .collect(),
                    required: vec![tag],
                })
            }
            Data::Union(_) => Err(syn::Error::new_spanned(
                input,
                "ToolInput requires a named struct or internally tagged enum",
            )),
        }
    }

    pub fn expand(&self, input: &DeriveInput) -> TokenStream {
        let name = &input.ident;
        let fields = self.fields.iter().map(|field| {
            let name = &field.name;
            let schema = &field.schema;
            quote!((#name.to_owned(), ::tools::tool_defs::ToolProperty::Schema(#schema)))
        });
        let field_names = self.fields.iter().map(|field| &field.name);
        let required = &self.required;
        let mut schema_generics = input.generics.clone();
        schema_generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote!(Self: serde::Serialize));
        let (schema_impl, schema_type, schema_where) = schema_generics.split_for_impl();
        let mut deserialize_generics = input.generics.clone();
        deserialize_generics
            .make_where_clause()
            .predicates
            .push(syn::parse_quote!(Self: serde::de::DeserializeOwned));
        let (deserialize_impl, deserialize_type, deserialize_where) =
            deserialize_generics.split_for_impl();
        quote! {
            impl #schema_impl ::tools::tool_defs::ToolInputSchema for #name #schema_type #schema_where {
                fn properties() -> ::utils::utils::FnvHashMap<String, ::tools::tool_defs::ToolProperty> {
                    [#(#fields),*].into_iter().collect()
                }

                fn required() -> Vec<String> {
                    vec![#(#required.to_owned()),*]
                }

                fn req(&self) -> anyhow::Result<::utils::utils::FnvHashMap<String, String>> {
                    let fields: ::utils::utils::FnvHashMap<String, serde_json::Value> =
                        serde_json::from_value(serde_json::to_value(self)?)?;
                    let names: &[&str] = &[#(#field_names),*];
                    Ok(fields.into_iter()
                        .filter(|(name, value)| names.contains(&name.as_str()) && !value.is_null())
                        .map(|(name, value)| {
                            let value = match value {
                                serde_json::Value::String(value) => value,
                                value => value.to_string(),
                            };
                            (name, value)
                        })
                        .collect())
                }
            }

            impl #deserialize_impl ::tools::tool_defs::LenientDeserialize for #name #deserialize_type #deserialize_where {
                fn deserialize_lenient(value: serde_json::Value) -> anyhow::Result<Self> {
                    match value {
                        value @ serde_json::Value::Object(_) => Ok(serde_json::from_value(value)?),
                        _ => Err(anyhow::anyhow!("expected JSON object")),
                    }
                }
            }
        }
    }
}

struct VariantSchema {
    name: String,
    fields: Vec<FieldSchema>,
}

enum VariantNaming {
    Unchanged,
    SnakeCase,
}

impl VariantNaming {
    fn new(value: Option<String>, input: &DeriveInput) -> syn::Result<Self> {
        match value.as_deref() {
            None => Ok(Self::Unchanged),
            Some("snake_case") => Ok(Self::SnakeCase),
            Some(_) => Err(syn::Error::new_spanned(
                input,
                "ToolInput supports snake_case or explicit #[serde(rename = \"...\")] variant names",
            )),
        }
    }

    fn apply(&self, name: &str) -> String {
        match self {
            Self::Unchanged => name.to_owned(),
            Self::SnakeCase => name
                .chars()
                .enumerate()
                .flat_map(|(index, character)| {
                    (index > 0 && character.is_uppercase())
                        .then_some('_')
                        .into_iter()
                        .chain(std::iter::once(character.to_ascii_lowercase()))
                })
                .collect(),
        }
    }
}

struct FieldSchema {
    name: String,
    schema: TokenStream,
    required: bool,
}

impl FieldSchema {
    fn new(field: &Field, variant_field: bool) -> syn::Result<Self> {
        let options = Attributes::new(
            &field.attrs,
            "tool",
            &[
                "description",
                "required",
                "kind",
                "values",
                "minimum",
                "maximum",
            ],
        )?;
        let serde = Attributes::new(&field.attrs, "serde", &["rename", "default"])?;
        let name = serde
            .string("rename")?
            .unwrap_or_else(|| field.ident.as_ref().unwrap().unraw().to_string());
        let description = options.string("description")?.unwrap_or_default();
        let inner = optional_inner(&field.ty);
        let nullable = variant_field || inner.is_some();
        let ty = inner.unwrap_or(&field.ty);
        let kind = SchemaKind::new(options.string("kind")?, ty)?;
        let values = options.strings("values")?;
        let minimum = options.integer("minimum")?;
        let maximum = options.integer("maximum")?;
        match &kind {
            _ if !values.is_empty() && !matches!(kind, SchemaKind::String) => Err(
                syn::Error::new_spanned(field, "values(...) requires a string field"),
            ),
            _ if (minimum.is_some() || maximum.is_some())
                && !matches!(kind, SchemaKind::Integer | SchemaKind::Number) =>
            {
                Err(syn::Error::new_spanned(
                    field,
                    "minimum and maximum require a numeric field",
                ))
            }
            _ if minimum
                .zip(maximum)
                .is_some_and(|(minimum, maximum)| minimum > maximum) =>
            {
                Err(syn::Error::new_spanned(
                    field,
                    "minimum must not exceed maximum",
                ))
            }
            _ => Ok(()),
        }?;
        let kind_name = kind.name();
        let types = match nullable {
            true => quote!(serde_json::json!([#kind_name, "null"])),
            false => quote!(serde_json::json!(#kind_name)),
        };
        let nested = match kind {
            SchemaKind::Object => quote! {
                schema["properties"] = serde_json::Value::Object(
                    <#ty as ::tools::tool_defs::ToolInputSchema>::properties().into_iter()
                        .map(|(name, property)| (name, property.into_schema()))
                        .collect()
                );
                schema["required"] = serde_json::json!(<#ty as ::tools::tool_defs::ToolInputSchema>::required());
            },
            _ => quote!(),
        };
        let allowed = match values.is_empty() {
            true => quote!(),
            false => quote! {
                schema["enum"] = serde_json::Value::Array(
                    [#(#values),*].into_iter().map(serde_json::Value::from)
                        .chain(#nullable.then_some(serde_json::Value::Null)).collect()
                );
            },
        };
        let minimum = minimum.map(|value| quote!(schema["minimum"] = serde_json::json!(#value);));
        let maximum = maximum.map(|value| quote!(schema["maximum"] = serde_json::json!(#value);));
        Ok(Self {
            name,
            required: options.flag("required")? || (inner.is_none() && !serde.contains("default")),
            schema: quote!({
                let mut schema = serde_json::json!({"type": #types});
                schema["description"] = serde_json::json!(#description);
                #nested
                #allowed
                #minimum
                #maximum
                schema
            }),
        })
    }
}

enum SchemaKind {
    String,
    Integer,
    Number,
    Boolean,
    Object,
}

impl SchemaKind {
    fn new(override_kind: Option<String>, ty: &Type) -> syn::Result<Self> {
        match override_kind {
            Some(kind) => match kind.as_str() {
                "string" => Ok(Self::String),
                "integer" => Ok(Self::Integer),
                "number" => Ok(Self::Number),
                "boolean" => Ok(Self::Boolean),
                "object" => Ok(Self::Object),
                _ => Err(syn::Error::new_spanned(ty, "Unsupported tool schema kind")),
            },
            None => Ok(match ty {
                Type::Path(path) => match path
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .as_deref()
                {
                    Some("String" | "str") => Self::String,
                    Some(
                        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32"
                        | "i64" | "i128" | "isize",
                    ) => Self::Integer,
                    Some("f32" | "f64") => Self::Number,
                    Some("bool") => Self::Boolean,
                    _ => Self::Object,
                },
                _ => Self::Object,
            }),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Object => "object",
        }
    }
}

fn optional_inner(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .filter(|segment| segment.ident == "Option")
            .and_then(|segment| match &segment.arguments {
                PathArguments::AngleBracketed(arguments) => match arguments.args.first() {
                    Some(GenericArgument::Type(ty)) => Some(ty),
                    _ => None,
                },
                _ => None,
            }),
        _ => None,
    }
}

fn named_fields(
    fields: &Fields,
) -> syn::Result<&syn::punctuated::Punctuated<Field, syn::Token![,]>> {
    match fields {
        Fields::Named(fields) => Ok(&fields.named),
        _ => Err(syn::Error::new_spanned(
            fields,
            "ToolInput requires named fields or unit enum variants",
        )),
    }
}

#[cfg(test)]
#[path = "../tests/unit/input/tests.rs"]
mod tests;
