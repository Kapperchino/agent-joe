use attributes::Attributes;
use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

mod attributes;
mod input;

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
    let options = Attributes::new(&input.attrs, "tool", &["name", "description"])?;
    let name = options
        .string("name")?
        .ok_or_else(|| syn::Error::new_spanned(input, "Missing tool name"))?;
    let description = options
        .string("description")?
        .ok_or_else(|| syn::Error::new_spanned(input, "Missing tool description"))?;
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
