use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};

use crate::Errors;
use crate::term::Term;

use super::{Built, opt_int};

fn last_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

pub fn duration_unit(wire: &str, ty: &syn::Type) -> Option<&'static str> {
    let int = matches!(
        last_ident(ty).as_deref(),
        Some("u16" | "u32" | "u64" | "usize")
    );
    match () {
        _ if int && wire.ends_with("_secs") => Some("secs"),
        _ if int && wire.ends_with("_ms") => Some("ms"),
        _ => None,
    }
}

pub fn tag_of(t: &Term) -> String {
    match t.name() {
        Some("enum_" | "enum_values") => "enum".into(),
        Some("union" | "union_") => "union".into(),
        Some("ref_") => "ref".into(),
        Some(n) => n.to_string(),
        None => String::new(),
    }
}

pub fn reject_redundant(
    field: &syn::Ident,
    t: &Term,
    ty: &syn::Type,
    duration: Option<&'static str>,
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) {
    let p = crate::p();
    if let (Some(unit), Some("duration")) = (duration, t.name()) {
        let positional = Term::positional(t.args());
        let only_unit = t.args().len() <= 1
            && positional
                .first()
                .and_then(|a| a.as_word())
                .is_none_or(|u| u == unit);
        if only_unit {
            errors.push(
                t.span(),
                "redundant ty: the `_secs` or `_ms` name already makes this a duration; remove `ty = ...`",
            );
        }
        return;
    }
    if !matches!(t, Term::Word(..)) {
        return;
    }
    let tag = tag_of(t);
    let span = t.span();
    let message = format!(
        "redundant ty on `{field}`: its Rust type already gives the semantic type `{tag}`; remove `ty = ...`"
    );
    checks.push(quote_spanned!(span=> const _: () = #p::not_redundant(<#ty as #p::DefaultSem>::SEM, #tag, #message);));
}

pub fn primitive_int(ty: &syn::Type) -> Option<syn::Ident> {
    let syn::Type::Path(tp) = ty else {
        return None;
    };
    let seg = tp.path.segments.last()?;
    let ints = [
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize",
    ];
    (tp.path.segments.len() == 1
        && seg.arguments.is_none()
        && ints.contains(&seg.ident.to_string().as_str()))
    .then(|| seg.ident.clone())
}

pub fn infer(
    ty: &syn::Type,
    range: (Option<i64>, Option<i64>),
    duration: Option<&'static str>,
    field: &syn::Ident,
    errors: &mut Errors,
) -> Built {
    let p = crate::p();
    let span = field.span();
    let (mn, mx) = (opt_int(range.0), opt_int(range.1));
    if let Some(unit) = duration {
        let unit = if unit == "ms" {
            quote!(#p::DurationUnit::Ms)
        } else {
            quote!(#p::DurationUnit::Secs)
        };
        return Built {
            node: quote!(#p::Node::new(#p::SemType::Duration(#p::DurationC { unit: #unit, min: #mn, max: #mx }))),
            checks: Vec::new(),
        };
    }
    let node = match last_ident(ty).as_deref() {
        Some("Vec" | "BTreeMap" | "HashMap") => {
            errors.push(
                span,
                format!("the semantic type of `{field}` cannot be inferred from a collection; add #[admin(ty = list(...))] or #[admin(ty = map(entity))]"),
            );
            quote!(#p::Node::default())
        }
        Some(_) => quote!({
            let mut n = <#ty as #p::DefaultSem>::default_node();
            #p::apply_range(&mut n, #mn, #mx);
            n
        }),
        None => {
            errors.push(
                span,
                format!(
                    "the semantic type of `{field}` cannot be inferred; add #[admin(ty = ...)]"
                ),
            );
            quote!(#p::Node::default())
        }
    };
    Built {
        node,
        checks: Vec::new(),
    }
}
