use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields};

use crate::sem;
use crate::serde_attrs;
use crate::term::Term;
use crate::{
    Errors, admin_attrs, assigned, check_keys, find, has_flag, humanize, p, redundant, skip_reason,
    str_of,
};

const STRUCT_KEYS: &[&str] = &["entity", "label", "plural", "groups", "skip", "tcp_only"];
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

fn field_paths(
    name: &syn::Ident,
    fields: &syn::FieldsNamed,
    wires: &[String],
    errors: &mut Errors,
) -> TokenStream {
    let f = quote!(::burrow_config::field);
    let mut consts = Vec::new();
    let mut entries = Vec::new();
    for (field, wire) in fields.named.iter().zip(wires) {
        let ty = sem::strip(&field.ty, "Option").unwrap_or(&field.ty);
        let konst = format_ident!("{}", wire.to_uppercase(), span = field.span());
        consts.push(quote! {
            pub const #konst: #f::Field<#name, #ty> = #f::Field::new(#wire);
        });
        let mut terms_errors = Errors::new();
        let terms = admin_attrs(&field.attrs, &mut terms_errors).unwrap_or_default();
        if let Some(Term::Call(kind, args, _)) = assigned(&terms, "ty")
            && kind == "map"
        {
            let (Some(entity), Some(value)) = (
                args.first().and_then(Term::as_word),
                sem::strip(&field.ty, "BTreeMap"),
            ) else {
                errors.push(
                    field.span(),
                    "map(entity) needs a BTreeMap<String, Entry> field",
                );
                continue;
            };
            entries.push(quote! {
                impl #f::Entry for #value {
                    const SECTION: &'static str = #wire;
                    const ENTITY: &'static str = #entity;
                }
            });
        }
    }
    quote! {
        impl #name {
            #(#consts)*
        }
        impl #f::Nest for #name {
            type Inner = #name;
        }
        #(#entries)*
    }
}

pub(super) fn expand_struct(
    input: &DeriveInput,
    s: &syn::DataStruct,
    errors: &mut Errors,
) -> TokenStream {
    let name = &input.ident;
    let p = p();
    let container = admin_attrs(&input.attrs, errors).unwrap_or_default();
    check_keys(&container, STRUCT_KEYS, errors);
    let serde_c = serde_attrs::container(&input.attrs);
    if let Some(reason) = skip_reason(&container, errors) {
        return quote! {
            impl #p::AdminSchema for #name {
                fn node() -> #p::Node {
                    let mut n = #p::Node::new(#p::SemType::Text(::core::default::Default::default()));
                    n.editable = #p::Editable::No { reason: #reason.into() };
                    n
                }
            }
            impl #p::Carrier<#p::sem::Object> for #name {}
        };
    }
    let Fields::Named(fields) = &s.fields else {
        errors.push(name.span(), "AdminSchema needs a struct with named fields");
        return TokenStream::new();
    };
    let field_names: Vec<String> = fields
        .named
        .iter()
        .map(|f| {
            serde_attrs::field(&f.attrs)
                .rename
                .unwrap_or_else(|| f.ident.as_ref().expect("named").to_string())
        })
        .collect();
    let paths = field_paths(name, fields, &field_names, errors);
    let mut groups: Vec<(String, String)> = Vec::new();
    if let Some(g) = find(&container, "groups") {
        for a in g.args() {
            match a {
                Term::Assign(k, v, _) if v.as_str().is_some() => {
                    groups.push((k.clone(), v.as_str().unwrap_or_default().to_string()))
                }
                other => errors.push(other.span(), "groups(...) takes key = \"Label\" pairs"),
            }
        }
    }
    let mut tcp_only: Vec<String> = Vec::new();
    if let Some(t) = find(&container, "tcp_only") {
        if !field_names.iter().any(|f| f == "protocol") {
            errors.push(
                t.span(),
                format!("`tcp_only` needs a `protocol` field in `{name}`"),
            );
        }
        for a in t.args() {
            match a.as_word() {
                Some(w) if field_names.iter().any(|f| f == w) => tcp_only.push(w.to_string()),
                Some(w) => errors.push(a.span(), format!("`{w}` is not a field of `{name}`")),
                None => errors.push(a.span(), "tcp_only(field, ...) takes field names"),
            }
        }
    }
    let mut pushes = Vec::new();
    let mut checks = Vec::new();
    for (f, wire) in fields.named.iter().zip(&field_names) {
        let ident = f.ident.as_ref().expect("named");
        let Some(terms) = admin_attrs(&f.attrs, errors) else {
            errors.push(
                f.span(),
                format!(
                    "field `{ident}` of `{name}` needs `#[admin]`, or `#[admin(skip = \"why\")]` if the admin UI must not edit it"
                ),
            );
            continue;
        };
        check_keys(&terms, FIELD_KEYS, errors);
        let serde_f = serde_attrs::field(&f.attrs);
        let schemars = serde_attrs::schemars(&f.attrs);
        let given_label = str_of(&terms, "label", errors);
        let label = given_label.clone().unwrap_or_else(|| humanize(wire));
        let help = str_of(&terms, "help", errors)
            .or(schemars.description.clone())
            .unwrap_or_default();
        if let Some(t) = find(&terms, "reduce_only") {
            let clash = ["skip", "locked", "readonly"]
                .into_iter()
                .find(|k| find(&terms, k).is_some());
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
        if let Some(reason) = skip_reason(&terms, errors) {
            pushes.push(quote! {
                {
                    let mut n = #p::Node::new(#p::SemType::Text(::core::default::Default::default()));
                    n.label = #label.into();
                    n.help = #help.into();
                    n.editable = #p::Editable::No { reason: #reason.into() };
                    fields.push(#p::Field { key: #wire.into(), node: n });
                }
            });
            continue;
        }
        redundant(&terms, &label, &humanize(wire), errors);
        let optional = sem::strip(&f.ty, "Option");
        let inner_ty = optional.unwrap_or(&f.ty);
        let range = (schemars.min, schemars.max);
        let duration = sem::duration_unit(wire, inner_ty);
        let built = match assigned(&terms, "ty") {
            Some(ty_term) => {
                sem::reject_redundant(ident, ty_term, inner_ty, duration, &mut checks, errors);
                sem::build(ty_term, Some(inner_ty), range, errors)
            }
            None => sem::infer(inner_ty, range, duration, ident, errors),
        };
        checks.extend(built.checks);
        let base = match sem::primitive_int(inner_ty) {
            Some(t) => {
                let node = built.node;
                quote!({
                    let mut n = #node;
                    #p::type_bounds(&mut n, #t::MIN as i128, #t::MAX as i128);
                    n
                })
            }
            None => built.node,
        };
        let mut meta = Vec::new();
        if let Some(g) = assigned(&terms, "group") {
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
        if has_flag(&terms, "advanced") {
            meta.push(quote!(n.advanced = true;));
        }
        if has_flag(&terms, "locked") {
            meta.push(quote!(n.locked = true;));
        }
        if has_flag(&terms, "reduce_only") {
            meta.push(quote!(n.reduce_only = true;));
        }
        match (find(&terms, "wide"), find(&terms, "narrow")) {
            (Some(t), Some(_)) => errors.push(t.span(), "`wide` and `narrow` contradict"),
            (Some(_), None) => meta.push(quote!(n.wide = ::core::option::Option::Some(true);)),
            (None, Some(_)) => meta.push(quote!(n.wide = ::core::option::Option::Some(false);)),
            (None, None) => {}
        }
        if has_flag(&terms, "danger_when_set") {
            meta.push(quote!(n.danger_when_set = true;));
        }
        if let Some(e) = str_of(&terms, "empty_label", errors) {
            meta.push(quote!(n.empty_label = ::core::option::Option::Some(#e.into());));
        }
        if let Some(r) = str_of(&terms, "readonly", errors) {
            meta.push(quote!(n.editable = #p::Editable::No { reason: #r.into() };));
        }
        if let Some(a) = assigned(&terms, "applies") {
            match a.as_word() {
                Some("live") => {
                    meta.push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Live);))
                }
                Some("reload") => meta
                    .push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Reload);)),
                Some("restart") => meta
                    .push(quote!(n.applies = ::core::option::Option::Some(#p::Applies::Restart);)),
                _ => errors.push(a.span(), "applies = live | reload | restart"),
            }
        }
        let mut relations = sem::relations(&terms, &field_names, name, wire, errors);
        if tcp_only.iter().any(|t| t == wire) {
            relations.push(quote!(#p::Relation::EnabledIf {
                cond: #p::Cond::Eq("protocol".into(), #p::json!("tcp")),
                reason: "UDP services do not support this".into(),
            }));
        }
        if !relations.is_empty() {
            meta.push(quote!(n.relations = ::std::vec![#(#relations),*];));
        }
        let has_default = serde_f.default.is_some() || serde_c.default;
        let required = optional.is_none() && !has_default;
        if required {
            meta.push(quote!(n.required = true;));
        }
        let default_expr = match (&serde_f.default, serde_c.default) {
            (Some(serde_attrs::Default::Path(path)), _) => {
                let path: syn::ExprPath = match syn::parse_str(path) {
                    Ok(p) => p,
                    Err(e) => {
                        errors.add(e);
                        continue;
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
        pushes.push(field_tokens);
    }
    let mut container_meta = Vec::new();
    if let Some(e) = assigned(&container, "entity") {
        match e.as_word() {
            Some(w) => container_meta
                .push(quote!(n.entity = ::core::option::Option::Some(#p::Entity::parse(#w));)),
            None => errors.push(e.span(), "entity = client | visitor | service"),
        }
    }
    if let Some(l) = str_of(&container, "label", errors) {
        container_meta.push(quote!(n.label = #l.into();));
    }
    let group_items = groups.iter().map(|(k, l)| {
        let (label, sub) = l.split_once(" | ").unwrap_or((l.as_str(), ""));
        quote!(#p::Group { key: #k.into(), label: #label.into(), sub: #sub.into() })
    });
    let group_consts = groups.iter().map(|(k, _)| {
        let c = format_ident!("GROUP_{}", k.to_uppercase());
        quote!(pub const #c: &'static str = #k;)
    });
    let check_fn = format_ident!("__admin_checks_{}", name);
    let tcp_only_const = if tcp_only.is_empty() {
        quote!()
    } else {
        quote! {
            impl #name {
                pub const TCP_ONLY: &'static [&'static str] = &[#(#tcp_only),*];
            }
        }
    };
    quote! {
        #tcp_only_const
        #paths
        impl #name {
            #(#group_consts)*
        }
        impl #p::DefaultSem for #name {
            const SEM: &'static str = "object";
            fn default_node() -> #p::Node {
                <#name as #p::AdminSchema>::node()
            }
        }
        impl #p::AdminSchema for #name {
            fn node() -> #p::Node {
                let mut fields: ::std::vec::Vec<#p::Field> = ::std::vec::Vec::new();
                #(#pushes)*
                let mut n = #p::Node::new(#p::SemType::Object(#p::ObjectC { fields }));
                n.groups = ::std::vec![#(#group_items),*];
                #(#container_meta)*
                n
            }
        }
        impl #p::Carrier<#p::sem::Object> for #name {}
        #[allow(non_snake_case, dead_code)]
        fn #check_fn() {
            #(#checks)*
        }
    }
}
