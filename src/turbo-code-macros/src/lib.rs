use attributes::Attributes;
use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

mod attributes;
mod input;
mod schema;

#[proc_macro_derive(ToolSchema, attributes(tool, serde))]
pub fn derive_tool_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match schema::expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(ToolDef, attributes(tool))]
pub fn derive_tool_def(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match tool_definition(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_derive(ToolInput, attributes(tool, serde))]
pub fn derive_tool_input(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match input::InputSchema::new(&input) {
        Ok(schema) => schema.expand(&input).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn tool_definition(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let options = Attributes::new(
        &input.attrs,
        "tool",
        &["name", "description", "input", "variants", "fields"],
    )?;
    let name = options
        .string("name")?
        .ok_or_else(|| syn::Error::new_spanned(input, "Missing tool name"))?;
    let description = options
        .string("description")?
        .ok_or_else(|| syn::Error::new_spanned(input, "Missing tool description"))?;
    match &input.data {
        Data::Struct(_) => Ok(()),
        _ => Err(syn::Error::new_spanned(input, "ToolDef requires a struct")),
    }?;
    match options.string("input")? {
        Some(input_type) => stateless_definition(
            input,
            &options,
            &name,
            &description,
            &syn::parse_str(&input_type)?,
        ),
        None if options.contains("variants") || options.contains("fields") => Err(
            syn::Error::new_spanned(input, "Schema filters require #[tool(input = \"Type\")]"),
        ),
        None => stateful_definition(input, &name, &description),
    }
}

fn stateless_definition(
    input: &DeriveInput,
    options: &Attributes,
    name: &str,
    description: &str,
    input_type: &syn::Type,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Data::Struct(data) = &input.data {
        for field in &data.fields {
            Attributes::new(&field.attrs, "tool", &[])?;
        }
    }
    let variants = options
        .string("variants")?
        .map(|value| syn::parse_str::<syn::Expr>(&value))
        .transpose()?
        .unwrap_or_else(|| syn::parse_quote!(&[]));
    let fields = options
        .string("fields")?
        .map(|value| syn::parse_str::<syn::Expr>(&value))
        .transpose()?
        .unwrap_or_else(|| syn::parse_quote!(&[]));
    let struct_name = &input.ident;
    let mut generics = input.generics.clone();
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#input_type: ::tools::tool_defs::schema::ToolSchema));
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let schema = quote!(::tools::tool_defs::schema::input_schema(<#input_type as ::tools::tool_defs::schema::ToolSchema>::schema(), #variants, #fields));
    Ok(quote! {
        impl #impl_generics ::tools::tool_defs::ToolDefTrait for #struct_name #type_generics #where_clause {
            fn tool_name() -> &'static str { #name }
            fn tool_description() -> &'static str { #description }
            fn field_properties() -> ::utils::utils::FnvHashMap<String, ::tools::tool_defs::ToolProperty> {
                #schema["properties"].as_object().unwrap().iter()
                    .map(|(name, schema)| (name.clone(), ::tools::tool_defs::ToolProperty::Schema(schema.clone())))
                    .collect()
            }
            fn required_fields() -> Vec<String> {
                #schema["required"].as_array().unwrap().iter()
                    .map(|name| name.as_str().unwrap().to_owned()).collect()
            }
            fn req(&self) -> anyhow::Result<::utils::utils::FnvHashMap<String, String>> {
                Ok(Default::default())
            }
        }
    })
}

fn stateful_definition(
    input: &DeriveInput,
    name: &str,
    description: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            _ => Err(syn::Error::new_spanned(
                input,
                "ToolDef requires named fields",
            )),
        },
        _ => Err(syn::Error::new_spanned(input, "ToolDef requires a struct")),
    }?;
    let inputs = fields
        .iter()
        .map(|field| {
            let options = Attributes::new(&field.attrs, "tool", &["input"])?;
            Ok(options.flag("input")?.then_some(field))
        })
        .collect::<syn::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let field = match inputs.as_slice() {
        [field] => Ok(*field),
        _ => Err(syn::Error::new_spanned(
            input,
            "ToolDef requires exactly one #[tool(input)] field",
        )),
    }?;
    let field_name = &field.ident;
    let field_type = &field.ty;
    let struct_name = &input.ident;
    let mut generics = input.generics.clone();
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(
            #field_type: ::tools::tool_defs::ToolInputSchema
        ));
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::tools::tool_defs::ToolDefTrait for #struct_name #type_generics #where_clause {
            fn tool_name() -> &'static str { #name }
            fn tool_description() -> &'static str { #description }

            fn field_properties() -> ::utils::utils::FnvHashMap<String, ::tools::tool_defs::ToolProperty> {
                <#field_type as ::tools::tool_defs::ToolInputSchema>::properties()
            }

            fn required_fields() -> Vec<String> {
                <#field_type as ::tools::tool_defs::ToolInputSchema>::required()
            }

            fn req(&self) -> anyhow::Result<::utils::utils::FnvHashMap<String, String>> {
                <#field_type as ::tools::tool_defs::ToolInputSchema>::req(&self.#field_name)
            }
        }
    })
}

#[cfg(test)]
#[path = "../tests/unit/definition/tests.rs"]
mod definition_tests;
