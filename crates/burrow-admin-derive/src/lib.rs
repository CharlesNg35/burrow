#![forbid(unsafe_code)]

mod sem;
mod serde_attrs;
mod term;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields};

use term::Term;

#[proc_macro_derive(AdminSchema, attributes(admin))]
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

fn p() -> TokenStream {
    quote!(::burrow::config::admin_schema::__private)
}

fn expand_struct(input: &DeriveInput, s: &syn::DataStruct, errors: &mut Errors) -> TokenStream {
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
        let base = built.node;
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

const VARIANT_KEYS: &[&str] = &["label", "help", "danger"];

fn expand_enum(input: &DeriveInput, e: &syn::DataEnum, errors: &mut Errors) -> TokenStream {
    let name = &input.ident;
    let p = p();
    let container = admin_attrs(&input.attrs, errors).unwrap_or_default();
    check_keys(&container, &["semantic", "skip", "label"], errors);
    let serde_c = serde_attrs::container(&input.attrs);
    let skipped = skip_reason(&container, errors);
    let mut options = Vec::new();
    let mut wires = Vec::new();
    let mut forms = Vec::new();
    for v in &e.variants {
        let vident = &v.ident;
        let terms = if skipped.is_some() {
            admin_attrs(&v.attrs, errors).unwrap_or_default()
        } else {
            match admin_attrs(&v.attrs, errors) {
                Some(t) => t,
                None => {
                    errors.push(
                        v.span(),
                        format!("variant `{vident}` of `{name}` needs #[admin]"),
                    );
                    continue;
                }
            }
        };
        check_keys(&terms, VARIANT_KEYS, errors);
        let wire = serde_attrs::field(&v.attrs).rename.unwrap_or_else(|| {
            serde_attrs::rename_variant(&vident.to_string(), serde_c.rename_all.as_deref())
        });
        let derived = wire.replace(['_', '-'], " ");
        let label = str_of(&terms, "label", errors).unwrap_or_else(|| derived.clone());
        redundant(&terms, &label, &derived, errors);
        let help = str_of(&terms, "help", errors).unwrap_or_default();
        let danger = has_flag(&terms, "danger");
        let form = label.clone();
        let pat = match &v.fields {
            Fields::Unit => quote!(#name::#vident),
            Fields::Unnamed(_) => quote!(#name::#vident(..)),
            Fields::Named(_) => quote!(#name::#vident { .. }),
        };
        if matches!(v.fields, Fields::Unit) {
            options.push(quote!(#p::EnumOption { value: #wire.into(), label: #label.into(), help: #help.into(), danger: #danger }));
            wires.push(quote!(#pat => #wire));
        }
        forms.push(quote!(#pat => #form));
    }
    let all_unit = e.variants.iter().all(|v| matches!(v.fields, Fields::Unit));
    let forms_fn = quote! {
        impl #name {
            #[allow(dead_code)]
            pub(crate) fn admin_form(&self) -> &'static str {
                match self {
                    #(#forms),*
                }
            }
        }
    };
    if let Some(reason) = skipped {
        return quote! {
            impl #p::AdminSchema for #name {
                fn node() -> #p::Node {
                    let mut n = #p::Node::new(#p::SemType::Text(::core::default::Default::default()));
                    n.editable = #p::Editable::No { reason: #reason.into() };
                    n
                }
            }
        };
    }
    if let Some(sem_term) = assigned(&container, "semantic") {
        let marker = match sem::marker(sem_term) {
            Some(m) => m,
            None => {
                errors.push(sem_term.span(), "unknown semantic type");
                return TokenStream::new();
            }
        };
        let built = sem::build(sem_term, None, (None, None), errors);
        let node = built.node;
        let marker = quote_spanned!(sem_term.span()=> #p::sem::#marker);
        let tag = sem::tag_of(sem_term);
        return quote! {
            impl #p::AdminSchema for #name {
                fn node() -> #p::Node {
                    #node
                }
            }
            impl #p::DefaultSem for #name {
                const SEM: &'static str = #tag;
                fn default_node() -> #p::Node {
                    <#name as #p::AdminSchema>::node()
                }
            }
            impl #p::Carrier<#marker> for #name {}
            #forms_fn
        };
    }
    if !all_unit {
        errors.push(
            name.span(),
            format!("enum `{name}` has data in its variants; describe the whole enum with #[admin(semantic = ...)]"),
        );
        return TokenStream::new();
    }
    quote! {
        impl #p::AdminEnum for #name {
            fn options() -> ::std::vec::Vec<#p::EnumOption> {
                ::std::vec![#(#options),*]
            }
            fn wire(&self) -> &'static str {
                match self {
                    #(#wires),*
                }
            }
        }
        impl #p::AdminSchema for #name {
            fn node() -> #p::Node {
                #p::Node::new(#p::SemType::Enum(#p::EnumC { options: <#name as #p::AdminEnum>::options() }))
            }
        }
        impl #p::Carrier<#p::sem::Enum> for #name {}
        impl #p::DefaultSem for #name {
            const SEM: &'static str = "enum";
            fn default_node() -> #p::Node {
                <#name as #p::AdminSchema>::node()
            }
        }
    }
}
