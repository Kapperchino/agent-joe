use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Fields, GenericArgument, LitStr,
    PathArguments, Type,
};

#[proc_macro_derive(ToolDef, attributes(tool))]
pub fn derive_tool_def(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_tool_def(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[proc_macro_derive(ToolInput, attributes(tool))]
pub fn derive_tool_input(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_tool_input(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn impl_tool_def(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;

    let tool_name = extract_field_value::<LitStr>(input, "name")?;
    let tool_description = extract_field_value::<LitStr>(input, "description")?;
    let (input_field, input_type) = extract_field_type(input, "input")?;

    Ok(quote! {
        impl crate::tool_defs::ToolDefTrait for #struct_name {
            fn tool_name() -> &'static str { #tool_name }
            fn tool_description() -> &'static str { #tool_description }

            fn field_properties() -> ::std::collections::HashMap<String, crate::claude::ToolProperty> {
                <#input_type as crate::tool_defs::ToolInputSchema>::properties()
            }

            fn required_fields() -> Vec<String> {
                <#input_type as crate::tool_defs::ToolInputSchema>::required()
            }

            fn req(&self) -> anyhow::Result<std::collections::HashMap<String,String>> {
                self.#input_field.req()
            }
        }
    })
}

fn impl_tool_input(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "ToolInput requires named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "ToolInput can only be derived for structs",
            ));
        }
    };

    let mut property_insertions = Vec::new();
    let mut required_names = Vec::new();
    let mut req_insertions = Vec::new();
    let mut lenient_fields = Vec::new();

    fields.iter().try_for_each(|field| {
        let field_name_str = field.ident.as_ref().unwrap().to_string();
        let field_name = &field.ident;
        let field_ty = &field.ty;

        // Generate lenient deserialization for every field
        if is_option_type(field_ty) {
            lenient_fields.push(quote! {
                #field_name: obj.get(#field_name_str)
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
            });
        } else {
            lenient_fields.push(quote! {
                #field_name: serde_json::from_value(
                    obj.get(#field_name_str)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                )?
            });
        }

        let description =
            extract_field_value_from_attrs_option::<LitStr>(&field.attrs, "description")?;
        let is_required = field_exist(&field.attrs, "required")?;

        if let Some(desc) = description {
            if is_required {
                required_names.push(field_name_str.clone());
            }

            let kind = infer_schema_kind(&field.ty);

            if kind == "object" {
                let inner_type = extract_inner_type(&field.ty);
                property_insertions.push(quote! {
                map.insert(#field_name_str.to_string(), crate::claude::ToolProperty::Object {
                    name: #field_name_str.to_string(),
                    prop_type: "object".to_string(),
                    description: #desc.to_string(),
                    properties: <#inner_type as crate::tool_defs::ToolInputSchema>::properties(),
                });
            });
                req_insertions.push(quote! {
                map.insert(#field_name_str.to_string(), serde_json::to_string(&#field_name_str)?);
            });
            } else {
                property_insertions.push(quote! {
                    map.insert(#field_name_str.to_string(), crate::claude::ToolProperty::Value {
                        name: #field_name_str.to_string(),
                        prop_type: #kind.to_string(),
                        description: #desc.to_string(),
                    });
                });
                req_insertions.push(quote! {
                    map.insert(#field_name_str.to_string(), self.#field_name.to_string());
                });
            }
        }
        Ok::<(), syn::Error>(())
    })?;

    Ok(quote! {
        impl crate::tool_defs::ToolInputSchema for #struct_name {
            fn properties() -> ::std::collections::HashMap<String, crate::claude::ToolProperty> {
                let mut map = ::std::collections::HashMap::new();
                #(#property_insertions)*
                map
            }

            fn required() -> Vec<String> {
                vec![#(#required_names.to_string()),*]
            }

            fn req(&self) -> anyhow::Result<std::collections::HashMap<String,String>> {
                let mut map = ::std::collections::HashMap::new();
                #(#req_insertions)*
                Ok(map)
            }
        }

        impl crate::tool_defs::LenientDeserialize for #struct_name {
            fn deserialize_lenient(s: &str) -> anyhow::Result<Self> {
                let value: serde_json::Value = serde_json::from_str(s)?;
                let obj = value.as_object()
                    .ok_or_else(|| anyhow::anyhow!("expected JSON object"))?;
                Ok(Self {
                    #(#lenient_fields),*
                })
            }
        }
    })
}

fn is_option_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}

fn extract_inner_type(ty: &Type) -> &Type {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner)) = args.args.first() {
                        return inner;
                    }
                }
            }
        }
    }
    ty
}

fn infer_schema_kind(ty: &Type) -> &'static str {
    let inner = extract_inner_type(ty);
    if let Type::Path(type_path) = inner {
        if let Some(segment) = type_path.path.segments.last() {
            return match segment.ident.to_string().as_str() {
                "String" | "str" => "string",
                "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
                | "i128" | "isize" => "integer",
                "f32" | "f64" => "number",
                "bool" => "boolean",
                _ => "object",
            };
        }
    }
    "object"
}

fn field_exist(attrs: &Vec<Attribute>, field_name: &str) -> Result<bool, syn::Error> {
    let res = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("tool"))
        .flat_map(|attr| {
            let mut exists = false;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident(field_name) {
                    exists = true;
                } else if meta.input.peek(syn::Token![=]) {
                    // consume `= value` for unrelated key-value pairs
                    meta.value()?.parse::<syn::Lit>()?;
                }
                Ok(())
            })?;
            Ok::<bool, syn::Error>(exists)
        })
        .collect::<Vec<_>>();
    Ok(res.into_iter().next().unwrap_or(false))
}

fn extract_field_value_from_attrs_option<T: syn::parse::Parse>(
    attrs: &Vec<Attribute>,
    field_name: &str,
) -> Result<Option<T>, syn::Error> {
    let res: Vec<T> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("tool"))
        .flat_map(|attr| {
            let mut parsed: Option<T> = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident(field_name) {
                    parsed = Some(meta.value()?.parse()?);
                } else if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<T>()?;
                }
                Ok(())
            })?;
            Ok::<Option<T>, syn::Error>(parsed)
        })
        .flatten()
        .collect::<Vec<_>>();
    Ok(res.into_iter().next())
}
fn extract_field_value_from_attrs<T: syn::parse::Parse>(
    attrs: &Vec<Attribute>,
    field_name: &str,
    input: &DeriveInput,
) -> Result<T, syn::Error> {
    let opts = extract_field_value_from_attrs_option::<T>(attrs, field_name)?;
    opts.ok_or_else(|| syn::Error::new(input.span(), format!("missing {field_name}")))
}

fn extract_field_value<T: syn::parse::Parse>(
    input: &DeriveInput,
    field_name: &str,
) -> Result<T, syn::Error> {
    extract_field_value_from_attrs(&input.attrs, field_name, input)
}

fn extract_field_type(
    input: &DeriveInput,
    field_name: &str,
) -> Result<(syn::Ident, Type), syn::Error> {
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            _ => Err(syn::Error::new_spanned(
                &input.ident,
                "ToolDef requires named fields",
            )),
        },
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            "ToolDef can only be derived for structs",
        )),
    }?;

    let res = fields
        .iter()
        .map(|field| (field, field.attrs.clone()))
        .flat_map(|(field, x)| {
            x.iter()
                .filter(|attr| attr.path().is_ident("tool"))
                .flat_map(|attr| {
                    let mut parsed: Option<(syn::Ident, Type)> = None;
                    attr.parse_nested_meta(|meta| {
                        if meta.path.is_ident(field_name) {
                            parsed = Some((field.ident.clone().unwrap(), field.ty.clone()));
                        } else if meta.input.peek(syn::Token![=]) {
                            meta.value()?.parse::<LitStr>()?;
                        }
                        Ok(())
                    })?;
                    Ok::<Option<(syn::Ident, Type)>, syn::Error>(parsed)
                })
                .flatten()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    res.into_iter()
        .next()
        .ok_or_else(|| syn::Error::new(input.span(), format!("missing {field_name}")))
}
