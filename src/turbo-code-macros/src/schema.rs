use crate::attributes::Attributes;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, Type, ext::IdentExt};

const CONSTRAINTS: &[(&str, &str)] = &[
    ("minimum", "minimum"),
    ("maximum", "maximum"),
    ("min_length", "minLength"),
    ("max_length", "maxLength"),
    ("min_items", "minItems"),
    ("max_items", "maxItems"),
    ("max_properties", "maxProperties"),
    ("default", "default"),
];

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let serde = Attributes::new(
        &input.attrs,
        "serde",
        &[
            "default",
            "deny_unknown_fields",
            "transparent",
            "tag",
            "rename_all",
            "try_from",
            "into",
        ],
    )?;
    let options = Attributes::new(&input.attrs, "tool", &["description"])?;
    let description = options.string("description")?.unwrap_or_default();
    let schema = match &input.data {
        Data::Struct(data) if serde.flag("transparent")? => {
            match data.fields.iter().collect::<Vec<_>>().as_slice() {
                [field] => field_schema(field),
                _ => Err(syn::Error::new_spanned(
                    input,
                    "Transparent schemas require one field",
                )),
            }
        }
        Data::Struct(data) => object(&data.fields, &serde),
        Data::Enum(data) => {
            let tag = serde.string("tag")?;
            let naming = crate::input::VariantNaming::new(serde.string("rename_all")?, input)?;
            let names = data
                .variants
                .iter()
                .map(|variant| {
                    let attributes = Attributes::new(&variant.attrs, "serde", &["rename"])?;
                    Ok(attributes
                        .string("rename")?
                        .unwrap_or_else(|| naming.apply(&variant.ident.unraw().to_string())))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            match names.is_empty()
                || names
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != names.len()
            {
                true => Err(syn::Error::new_spanned(
                    input,
                    "Schema variants must be distinct and nonempty",
                )),
                false => Ok(()),
            }?;
            match tag {
                Some(tag) => {
                    let variants = data.variants.iter().zip(names).map(|(variant, name)| {
                        let schema = match &variant.fields {
                            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => field_schema(&fields.unnamed[0]),
                            fields => object(fields, &serde),
                        }?;
                        Ok(quote!(::common_models::tool_schema::tagged(#schema, #tag, #name, #description)))
                    }).collect::<syn::Result<Vec<_>>>()?;
                    Ok(quote!(serde_json::json!({"oneOf": [#(#variants),*]})))
                }
                None if data
                    .variants
                    .iter()
                    .all(|variant| matches!(variant.fields, Fields::Unit)) =>
                {
                    Ok(quote!(
                        serde_json::json!({"type": "string", "enum": [#(#names),*]})
                    ))
                }
                None => Err(syn::Error::new_spanned(
                    input,
                    "Data enums require a serde tag",
                )),
            }
        }
        Data::Union(_) => Err(syn::Error::new_spanned(
            input,
            "Unions cannot be tool schemas",
        )),
    }?;
    let name = &input.ident;
    let mut generics = input.generics.clone();
    for parameter in generics.type_params_mut() {
        parameter
            .bounds
            .push(syn::parse_quote!(::common_models::tool_schema::ToolSchema));
    }
    let (implementation, types, bounds) = generics.split_for_impl();
    Ok(quote! {
        impl #implementation ::common_models::tool_schema::ToolSchema for #name #types #bounds {
            fn schema() -> serde_json::Value { #schema }
        }
    })
}

fn object(fields: &Fields, serde: &Attributes) -> syn::Result<TokenStream> {
    let fields = match fields {
        Fields::Named(fields) => Ok(fields.named.iter().collect::<Vec<_>>()),
        Fields::Unit => Ok(Vec::new()),
        _ => Err(syn::Error::new_spanned(
            fields,
            "Objects require named fields",
        )),
    }?;
    let mut properties = Vec::new();
    let mut required = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for field in fields {
        let options = field_options(field)?;
        let attributes = Attributes::new(
            &field.attrs,
            "serde",
            &["rename", "default", "skip_serializing_if"],
        )?;
        if !options.flag("skip")? {
            let name = attributes
                .string("rename")?
                .unwrap_or_else(|| field.ident.as_ref().unwrap().unraw().to_string());
            match names.insert(name.clone()) && serde.string("tag")?.as_deref() != Some(&name) {
                true => Ok(()),
                false => Err(syn::Error::new_spanned(
                    field,
                    "Duplicate or conflicting schema field",
                )),
            }?;
            let schema = field_schema(field)?;
            let optional = matches!(&field.ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Option"));
            if options.flag("required")?
                || !(optional
                    || options.flag("optional")?
                    || serde.contains("default")
                    || attributes.contains("default"))
            {
                required.push(name.clone());
            }
            properties.push(quote!((#name.to_owned(), #schema)));
        }
    }
    let additional = serde
        .flag("deny_unknown_fields")?
        .then(|| quote!(schema["additionalProperties"] = serde_json::json!(false);));
    Ok(quote!({
        let properties: serde_json::Map<String, serde_json::Value> = [#(#properties),*].into_iter().collect();
        let mut schema = serde_json::json!({"type": "object", "properties": properties, "required": [#(#required),*]});
        #additional
        schema
    }))
}

fn field_options(field: &Field) -> syn::Result<Attributes> {
    let allowed = [
        "description",
        "required",
        "optional",
        "skip",
        "nullable",
        "schema",
        "items",
        "additional_properties",
    ]
    .into_iter()
    .chain(CONSTRAINTS.iter().map(|(name, _)| *name))
    .collect::<Vec<_>>();
    let options = Attributes::new(&field.attrs, "tool", &allowed)?;
    match options.flag("required")? && options.flag("optional")? {
        true => Err(syn::Error::new_spanned(
            field,
            "A field cannot be both required and optional",
        )),
        false => Ok(options),
    }
}

fn constraints(options: &Attributes) -> syn::Result<Vec<TokenStream>> {
    for (minimum, maximum) in [
        ("minimum", "maximum"),
        ("min_length", "max_length"),
        ("min_items", "max_items"),
    ] {
        let minimum = options
            .expression(minimum)?
            .map(literal_bound)
            .transpose()?
            .flatten();
        let maximum = options
            .expression(maximum)?
            .map(literal_bound)
            .transpose()?
            .flatten();
        match minimum.zip(maximum) {
            Some((minimum, maximum)) if minimum > maximum => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "Minimum constraint must not exceed maximum",
            )),
            _ => Ok(()),
        }?;
    }
    CONSTRAINTS
        .iter()
        .map(|(name, key)| {
            if *name != "default" {
                options.expression(name)?.map(literal_bound).transpose()?;
            }
            Ok(options
                .expression(name)?
                .map(|value| quote!(schema[#key] = serde_json::json!(#value);)))
        })
        .collect::<syn::Result<Vec<_>>>()
        .map(|values| values.into_iter().flatten().collect())
}

fn literal_bound(expression: &syn::Expr) -> syn::Result<Option<u64>> {
    match expression {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Int(value),
            ..
        }) => value.base10_parse().map(Some),
        syn::Expr::Path(_) => Ok(None),
        expression => Err(syn::Error::new_spanned(
            expression,
            "Expected a nonnegative integer or constant path",
        )),
    }
}

fn field_schema(field: &Field) -> syn::Result<TokenStream> {
    let options = field_options(field)?;
    let ty = options
        .string("schema")?
        .map(|ty| syn::parse_str::<Type>(&ty))
        .transpose()?
        .unwrap_or_else(|| field.ty.clone());
    let description = options
        .string("description")?
        .map(|value| quote!(schema["description"] = serde_json::json!(#value);));
    let constraints = constraints(&options)?;
    let nested = ["items", "additional_properties"]
        .into_iter()
        .map(|name| {
            let attributes = options.nested(
                name,
                &CONSTRAINTS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
            )?;
            let constraints = self::constraints(&attributes)?;
            let key = match name {
                "additional_properties" => "additionalProperties",
                name => name,
            };
            Ok(match constraints.is_empty() {
                true => quote!(),
                false => quote!({ let schema = &mut schema[#key]; #(#constraints)* }),
            })
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let nullable = options
        .flag("nullable")?
        .then(|| quote!(schema = ::common_models::tool_schema::nullable(schema);));
    Ok(quote!({
        let mut schema = <#ty as ::common_models::tool_schema::ToolSchema>::schema();
        #description
        #(#constraints)*
        #(#nested)*
        #nullable
        schema
    }))
}

#[cfg(test)]
#[path = "../tests/unit/schema/tests.rs"]
mod tests;
