#![forbid(unsafe_code)]

mod enums;
mod sem;
mod serde_attrs;
mod structs;
mod term;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Data, DeriveInput};

use enums::expand_enum;
use structs::expand_struct;
use term::Term;

#[proc_macro_derive(AdminSchema, attributes(admin, schemars))]
pub fn derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

pub(crate) struct Errors(Option<syn::Error>);

impl Errors {
    fn new() -> Self {
        Errors(None)
    }

    pub(crate) fn push(&mut self, span: Span, msg: impl std::fmt::Display) {
        let e = syn::Error::new(span, msg);
        match &mut self.0 {
            Some(all) => all.combine(e),
            None => self.0 = Some(e),
        }
    }

    fn add(&mut self, e: syn::Error) {
        match &mut self.0 {
            Some(all) => all.combine(e),
            None => self.0 = Some(e),
        }
    }

    fn finish(self) -> syn::Result<()> {
        match self.0 {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

fn admin_attrs(attrs: &[syn::Attribute], errors: &mut Errors) -> Option<Vec<Term>> {
    let mut found: Option<Vec<Term>> = None;
    for a in attrs.iter().filter(|a| a.path().is_ident("admin")) {
        match term::parse_attr(a) {
            Ok(t) => found.get_or_insert_with(Vec::new).extend(t),
            Err(e) => {
                errors.add(e);
                found.get_or_insert_with(Vec::new);
            }
        }
    }
    found
}

fn check_keys(terms: &[Term], allowed: &[&str], errors: &mut Errors) {
    for t in terms {
        match t.name() {
            Some(n) if allowed.contains(&n) => {}
            Some(n) => errors.push(
                t.span(),
                format!(
                    "unknown key `{n}` in #[admin(...)]; expected one of: {}",
                    allowed.join(", ")
                ),
            ),
            None => errors.push(
                t.span(),
                "expected `key = value` or a flag in #[admin(...)]",
            ),
        }
    }
}

fn find<'a>(terms: &'a [Term], key: &str) -> Option<&'a Term> {
    terms.iter().find(|t| t.name() == Some(key))
}

fn assigned<'a>(terms: &'a [Term], key: &str) -> Option<&'a Term> {
    terms.iter().find_map(|t| match t {
        Term::Assign(n, v, _) if n == key => Some(&**v),
        _ => None,
    })
}

fn has_flag(terms: &[Term], key: &str) -> bool {
    terms.iter().any(|t| t.as_word() == Some(key))
}

fn str_of(terms: &[Term], key: &str, errors: &mut Errors) -> Option<String> {
    let v = assigned(terms, key)?;
    match v.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            errors.push(
                v.span(),
                format!("`{key}` needs a string, like {key} = \"...\""),
            );
            None
        }
    }
}

fn skip_reason(terms: &[Term], errors: &mut Errors) -> Option<String> {
    let t = find(terms, "skip")?;
    match t {
        Term::Assign(_, v, _) => match v.as_str() {
            Some(s) if !s.trim().is_empty() => Some(s.to_string()),
            _ => {
                errors.push(t.span(), "`skip` needs a reason: #[admin(skip = \"why\")]");
                Some(String::new())
            }
        },
        _ => {
            errors.push(t.span(), "`skip` needs a reason: #[admin(skip = \"why\")]");
            Some(String::new())
        }
    }
}

fn redundant(terms: &[Term], label: &str, derived: &str, errors: &mut Errors) {
    if let Some(t) = assigned(terms, "label")
        && label == derived
    {
        errors.push(
            t.span(),
            format!("redundant label: \"{derived}\" is already derived from the name; remove `label = ...`"),
        );
    }
}

pub(crate) fn humanize(name: &str) -> String {
    let words = name.replace('_', " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let out = match &input.data {
        Data::Struct(s) => expand_struct(input, s, &mut errors),
        Data::Enum(e) => expand_enum(input, e, &mut errors),
        Data::Union(_) => {
            errors.push(
                input.ident.span(),
                "AdminSchema cannot be derived for unions",
            );
            TokenStream::new()
        }
    };
    errors.finish()?;
    Ok(out)
}

fn p() -> TokenStream {
    quote!(::burrow_config::admin_schema::__private)
}
