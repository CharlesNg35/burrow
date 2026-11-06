use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields};

use crate::field::{Owner, field_push};
use crate::sem;
use crate::serde_attrs;
use crate::term::Term;
use crate::{Errors, admin_attrs, assigned, check_keys, find, p, skip_reason, str_of};

const STRUCT_KEYS: &[&str] = &[
    "entity", "label", "plural", "groups", "skip", "tcp_only", "one_of",
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
            && Term::value(args, "key").is_none()
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
    let groups = declared_groups(&container, errors);
    let tcp_only = tcp_only_fields(&container, &field_names, name, errors);
    let mut pushes = Vec::new();
    let mut checks = Vec::new();
    let owner = Owner {
        name,
        p: &p,
        field_names: &field_names,
        groups: &groups,
        tcp_only: &tcp_only,
        default: serde_c.default,
    };
    for (f, wire) in fields.named.iter().zip(&field_names) {
        if let Some(tokens) = field_push(&owner, f, wire, &mut checks, errors) {
            pushes.push(tokens);
        }
    }
    struct_impls(&owner, &container, paths, &pushes, &checks, errors)
}

fn declared_groups(container: &[Term], errors: &mut Errors) -> Vec<(String, String)> {
    let mut groups: Vec<(String, String)> = Vec::new();
    if let Some(g) = find(container, "groups") {
        for a in g.args() {
            match a {
                Term::Assign(k, v, _) if v.as_str().is_some() => {
                    groups.push((k.clone(), v.as_str().unwrap_or_default().to_string()))
                }
                other => errors.push(other.span(), "groups(...) takes key = \"Label\" pairs"),
            }
        }
    }
    groups
}

fn one_of_groups(
    container: &[Term],
    field_names: &[String],
    errors: &mut Errors,
) -> Vec<TokenStream> {
    let p = p();
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for t in container.iter().filter(|t| t.name() == Some("one_of")) {
        let mut key = None;
        let mut required = false;
        let mut choices: Vec<Vec<String>> = Vec::new();
        for a in t.args() {
            match a {
                Term::Assign(k, v, _) if v.as_str().is_some() => {
                    key = Some((k.clone(), v.as_str().unwrap_or_default().to_string()));
                }
                Term::Word(w, _) if w == "required" => required = true,
                Term::Word(w, _) => choices.push(vec![w.clone()]),
                Term::Call(w, rest, _) => choices.push(
                    std::iter::once(w.clone())
                        .chain(rest.iter().filter_map(|r| r.as_word().map(str::to_string)))
                        .collect(),
                ),
                other => errors.push(
                    other.span(),
                    "one_of(key = \"Label | help\", required, field, field(with, these), ...)",
                ),
            }
        }
        for k in choices.iter().flatten() {
            if !field_names.contains(k) {
                errors.push(t.span(), format!("`{k}` is not a field of this struct"));
            }
            if seen.contains(k) {
                errors.push(t.span(), format!("`{k}` is in two one_of groups"));
            }
            seen.push(k.clone());
        }
        let Some((key, label)) = key.filter(|_| choices.len() >= 2) else {
            errors.push(
                t.span(),
                "one_of needs key = \"Label\" and at least two choices",
            );
            continue;
        };
        let (label, help) = label.split_once(" | ").unwrap_or((label.as_str(), ""));
        let lists = choices
            .iter()
            .map(|c| quote!(::std::vec![#(#c.to_string()),*]));
        out.push(quote!(#p::OneOf {
            key: #key.into(),
            label: #label.into(),
            help: #help.into(),
            required: #required,
            choices: ::std::vec![#(#lists),*],
        }));
    }
    out
}

fn tcp_only_fields(
    container: &[Term],
    field_names: &[String],
    name: &syn::Ident,
    errors: &mut Errors,
) -> Vec<String> {
    let mut tcp_only: Vec<String> = Vec::new();
    if let Some(t) = find(container, "tcp_only") {
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
    tcp_only
}

fn struct_impls(
    o: &Owner,
    container: &[Term],
    paths: TokenStream,
    pushes: &[TokenStream],
    checks: &[TokenStream],
    errors: &mut Errors,
) -> TokenStream {
    let (name, p, groups, tcp_only) = (o.name, o.p, o.groups, o.tcp_only);
    let one_of = one_of_groups(container, o.field_names, errors);
    let mut container_meta = Vec::new();
    if let Some(e) = assigned(container, "entity") {
        match e.as_word() {
            Some(w) => container_meta
                .push(quote!(n.entity = ::core::option::Option::Some(#p::Entity::parse(#w));)),
            None => errors.push(e.span(), "entity = client | visitor | service"),
        }
    }
    if let Some(l) = str_of(container, "label", errors) {
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
                let one_of = ::std::vec![#(#one_of),*];
                let mut n = #p::Node::new(#p::SemType::Object(#p::ObjectC { fields, one_of }));
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
