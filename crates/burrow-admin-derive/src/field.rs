use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;

use crate::sem;
use crate::serde_attrs;
use crate::term::Term;
use crate::{
    Errors, admin_attrs, assigned, check_keys, find, has_flag, humanize, redundant, skip_reason,
    str_of,
};

const FIELD_KEYS: &[&str] = &[
    "ty",
    "skip",
    "label",
    "help",
    "group",
    "advanced",
    "danger_when_set",
    "locked",
    "reduce_only",
    "wide",
    "narrow",
    "none_label",
    "empty_label",
    "applies",
    "one_of",
    "visible_if",
    "enabled_if",
    "options_filter",
    "inverse",
    "default_from",
    "cross",
    "same_len",
    "warn_if",
    "readonly",
];

pub(crate) struct Owner<'a> {
    pub(crate) name: &'a syn::Ident,
    pub(crate) p: &'a TokenStream,
    pub(crate) field_names: &'a [String],
    pub(crate) groups: &'a [(String, String)],
    pub(crate) tcp_only: &'a [String],
    pub(crate) default: bool,
}

pub(crate) fn field_push(
    o: &Owner,
    f: &syn::Field,
    wire: &str,
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) -> Option<TokenStream> {
    let (name, p) = (o.name, o.p);
    let ident = f.ident.as_ref().expect("named");
    let Some(terms) = admin_attrs(&f.attrs, errors) else {
        errors.push(
            f.span(),
            format!(
                "field `{ident}` of `{name}` needs `#[admin]`, or `#[admin(skip = \"why\")]` if the admin UI must not edit it"
            ),
        );
        return None;
    };
    check_keys(&terms, FIELD_KEYS, errors);
    let serde_f = serde_attrs::field(&f.attrs);
    let schemars = serde_attrs::schemars(&f.attrs);
    let given_label = str_of(&terms, "label", errors);
    let label = given_label.clone().unwrap_or_else(|| humanize(wire));
    let help = str_of(&terms, "help", errors)
        .or(schemars.description.clone())
        .unwrap_or_default();
    check_reduce_only(&terms, errors);
    if let Some(reason) = skip_reason(&terms, errors) {
        return Some(quote! {
            {
                let mut n = #p::Node::new(#p::SemType::Text(::core::default::Default::default()));
                n.label = #label.into();
                n.help = #help.into();
                n.editable = #p::Editable::No { reason: #reason.into() };
                fields.push(#p::Field { key: #wire.into(), node: n });
            }
        });
    }
    redundant(&terms, &label, &humanize(wire), errors);
    let optional = sem::strip(&f.ty, "Option");
    let inner_ty = optional.unwrap_or(&f.ty);
    let range = (schemars.min, schemars.max);
    let base = field_base(ident, &terms, inner_ty, wire, range, checks, errors);
    let mut meta = field_meta(o, &terms, errors);
    meta.extend(field_relations(o, &terms, wire, errors));
    let has_default = serde_f.default.is_some() || o.default;
    let required = optional.is_none() && !has_default;
    if required {
        meta.push(quote!(n.required = true;));
    }
    let default_expr = field_default(o, f, &serde_f, errors)?;
    if let Some(d) = default_expr {
        meta.push(quote!(n.default = #p::scalar_default(#d);));
    }
    let field_tokens = if optional.is_some() {
        let none_label = str_of(&terms, "none_label", errors);
        let none = match none_label {
            Some(l) => quote!(::core::option::Option::Some(#l.into())),
            None => quote!(::core::option::Option::None),
        };
        quote! {
            {
                let inner = #base;
                let mut n = #p::Node::new(#p::SemType::Optional(#p::OptionalC { inner: ::std::boxed::Box::new(inner) }));
                n.none_label = #none;
                n.label = #label.into();
                n.help = #help.into();
                #(#meta)*
                fields.push(#p::Field { key: #wire.into(), node: n });
            }
        }
    } else {
        if let Some(t) = find(&terms, "none_label") {
            errors.push(t.span(), "`none_label` is only for Option fields");
        }
        quote! {
            {
                let mut n = #base;
                n.label = #label.into();
                n.help = #help.into();
                #(#meta)*
                fields.push(#p::Field { key: #wire.into(), node: n });
            }
        }
    };
    Some(field_tokens)
}

fn check_reduce_only(terms: &[Term], errors: &mut Errors) {
    if let Some(t) = find(terms, "reduce_only") {
        let clash = ["skip", "locked", "readonly"]
            .into_iter()
            .find(|k| find(terms, k).is_some());
        match (t.as_word(), clash) {
            (None, _) => errors.push(t.span(), "`reduce_only` is a flag: #[admin(reduce_only)]"),
            (Some(_), Some(k)) => errors.push(
                t.span(),
                format!(
                    "`reduce_only` and `{k}` contradict: a reduce_only field is edited in the admin UI, but only towards less power"
                ),
            ),
            (Some(_), None) => {}
        }
    }
}

fn field_base(
    ident: &syn::Ident,
    terms: &[Term],
    inner_ty: &syn::Type,
    wire: &str,
    range: (Option<i64>, Option<i64>),
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) -> TokenStream {
    let p = crate::p();
    let duration = sem::duration_unit(wire, inner_ty);
    let built = match assigned(terms, "ty") {
        Some(ty_term) => {
            sem::reject_redundant(ident, ty_term, inner_ty, duration, checks, errors);
            sem::build(ty_term, Some(inner_ty), range, errors)
        }
        None => sem::infer(inner_ty, range, duration, ident, errors),
    };
    checks.extend(built.checks);
    match sem::primitive_int(inner_ty) {
        Some(t) => {
            let node = built.node;
            quote!({
                let mut n = #node;
                #p::type_bounds(&mut n, #t::MIN as i128, #t::MAX as i128);
                n
            })
        }
        None => built.node,
    }
}

fn field_meta(o: &Owner, terms: &[Term], errors: &mut Errors) -> Vec<TokenStream> {
    let (name, p, groups) = (o.name, o.p, o.groups);
    let mut meta = Vec::new();
    if let Some(g) = assigned(terms, "group") {
        match g.as_word() {
            Some(w) if groups.is_empty() || groups.iter().any(|(k, _)| k == w) => {
                meta.push(quote!(n.group = #w.into();))
            }
            Some(w) => errors.push(
                g.span(),
                format!("group `{w}` is not declared in #[admin(groups(...))] of `{name}`"),
            ),
            None => errors.push(g.span(), "group = name"),
        }
    }
    if has_flag(terms, "advanced") {
        meta.push(quote!(n.advanced = true;));
    }
    if has_flag(terms, "locked") {
        meta.push(quote!(n.locked = true;));
    }
    if has_flag(terms, "reduce_only") {
        meta.push(quote!(n.reduce_only = true;));
    }
    match (find(terms, "wide"), find(terms, "narrow")) {
        (Some(t), Some(_)) => errors.push(t.span(), "`wide` and `narrow` contradict"),
        (Some(_), None) => meta.push(quote!(n.wide = ::core::option::Option::Some(true);)),
        (None, Some(_)) => meta.push(quote!(n.wide = ::core::option::Option::Some(false);)),
        (None, None) => {}
    }
    if has_flag(terms, "danger_when_set") {
        meta.push(quote!(n.danger_when_set = true;));
    }
    if let Some(e) = str_of(terms, "empty_label", errors) {
        meta.push(quote!(n.empty_label = ::core::option::Option::Some(#e.into());));
    }
    if let Some(r) = str_of(terms, "readonly", errors) {
        meta.push(quote!(n.editable = #p::Editable::No { reason: #r.into() };));
    }
    if let Some(a) = assigned(terms, "applies") {
        match a.as_word() {
            Some("live") => {
                meta.push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Live);))
            }
            Some("reload") => {
                meta.push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Reload);))
            }
            Some("restart") => {
                meta.push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Restart);))
            }
            _ => errors.push(a.span(), "applies = live | reload | restart"),
        }
    }
    meta
}

fn field_relations(
    o: &Owner,
    terms: &[Term],
    wire: &str,
    errors: &mut Errors,
) -> Option<TokenStream> {
    let p = o.p;
    let mut relations = sem::relations(terms, o.field_names, o.name, wire, errors);
    if o.tcp_only.iter().any(|t| t == wire) {
        relations.push(quote!(#p::Relation::EnabledIf {
            cond: #p::Cond::Eq("protocol".into(), #p::json!("tcp")),
            reason: "UDP services do not support this".into(),
        }));
    }
    if !relations.is_empty() {
        return Some(quote!(n.relations = ::std::vec![#(#relations),*];));
    }
    None
}

fn field_default(
    o: &Owner,
    f: &syn::Field,
    serde_f: &serde_attrs::Field,
    errors: &mut Errors,
) -> Option<Option<TokenStream>> {
    let (name, p) = (o.name, o.p);
    let ident = f.ident.as_ref().expect("named");
    let default_expr = match (&serde_f.default, o.default) {
        (Some(serde_attrs::Default::Path(path)), _) => {
            let path: syn::ExprPath = match syn::parse_str(path) {
                Ok(p) => p,
                Err(e) => {
                    errors.add(e);
                    return None;
                }
            };
            Some(quote!(#p::to_json(&#path())))
        }
        (_, true) => {
            Some(quote!(#p::to_json(&<#name as ::core::default::Default>::default().#ident)))
        }
        (Some(serde_attrs::Default::Plain), false) => {
            let ty = &f.ty;
            Some(quote!(#p::to_json(&<#ty as ::core::default::Default>::default())))
        }
        (None, false) => None,
    };
    Some(default_expr)
}
