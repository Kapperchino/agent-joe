use std::collections::BTreeMap;
use syn::{Attribute, Expr, ExprLit, Lit, LitStr, Meta, Token, punctuated::Punctuated};

pub(crate) struct Attributes {
    values: BTreeMap<String, Meta>,
}

impl Attributes {
    pub fn new(attrs: &[Attribute], namespace: &str, allowed: &[&str]) -> syn::Result<Self> {
        let values = attrs
            .iter()
            .filter(|attr| attr.path().is_ident(namespace))
            .try_fold(BTreeMap::new(), |values, attr| {
                attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
                    .into_iter()
                    .try_fold(values, |mut values, meta| {
                        let name = meta
                            .path()
                            .get_ident()
                            .ok_or_else(|| {
                                syn::Error::new_spanned(&meta, "Expected an attribute name")
                            })?
                            .to_string();
                        match name {
                            name if !allowed.contains(&name.as_str()) => {
                                Err(syn::Error::new_spanned(
                                    &meta,
                                    format!("Unsupported {namespace} attribute: {name}"),
                                ))
                            }
                            name if values.contains_key(&name) => Err(syn::Error::new_spanned(
                                &meta,
                                format!("Duplicate {namespace} attribute: {name}"),
                            )),
                            name => {
                                values.insert(name, meta);
                                Ok(values)
                            }
                        }
                    })
            })?;
        Ok(Self { values })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn flag(&self, name: &str) -> syn::Result<bool> {
        match self.values.get(name) {
            None => Ok(false),
            Some(Meta::Path(_)) => Ok(true),
            Some(meta) => Err(syn::Error::new_spanned(
                meta,
                "Expected a flag without a value",
            )),
        }
    }

    pub fn string(&self, name: &str) -> syn::Result<Option<String>> {
        self.values
            .get(name)
            .map(|meta| match meta {
                Meta::NameValue(value) => match &value.value {
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(value),
                        ..
                    }) => Ok(value.value()),
                    value => Err(syn::Error::new_spanned(value, "Expected a string literal")),
                },
                meta => Err(syn::Error::new_spanned(meta, "Expected a string value")),
            })
            .transpose()
    }

    pub fn integer(&self, name: &str) -> syn::Result<Option<u64>> {
        self.values
            .get(name)
            .map(|meta| match meta {
                Meta::NameValue(value) => match &value.value {
                    Expr::Lit(ExprLit {
                        lit: Lit::Int(value),
                        ..
                    }) => value.base10_parse(),
                    value => Err(syn::Error::new_spanned(
                        value,
                        "Expected a nonnegative integer literal",
                    )),
                },
                meta => Err(syn::Error::new_spanned(meta, "Expected an integer value")),
            })
            .transpose()
    }

    pub fn strings(&self, name: &str) -> syn::Result<Vec<String>> {
        match self.values.get(name) {
            None => Ok(Vec::new()),
            Some(Meta::List(list)) => {
                let values =
                    list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
                match values.is_empty() {
                    true => Err(syn::Error::new_spanned(
                        list,
                        "Expected at least one allowed value",
                    )),
                    false => Ok(values.into_iter().map(|value| value.value()).collect()),
                }
            }
            Some(meta) => Err(syn::Error::new_spanned(
                meta,
                "Expected values(\"first\", \"second\")",
            )),
        }
    }
}
