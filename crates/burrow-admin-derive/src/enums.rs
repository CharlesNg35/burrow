use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{DeriveInput, Fields};

use crate::sem;
use crate::serde_attrs;
use crate::term::Term;
use crate::{
    Errors, admin_attrs, assigned, check_keys, has_flag, p, redundant, skip_reason, str_of,
};

const VARIANT_KEYS: &[&str] = &["label", "help", "danger"];

struct Arms {
    options: Vec<TokenStream>,
    wires: Vec<TokenStream>,
    parses: Vec<TokenStream>,
    forms: Vec<TokenStream>,
    units: Vec<TokenStream>,
}

pub(super) fn expand_enum(
    input: &DeriveInput,
    e: &syn::DataEnum,
    errors: &mut Errors,
) -> TokenStream {
    let name = &input.ident;
    let p = p();
    let container = admin_attrs(&input.attrs, errors).unwrap_or_default();
    check_keys(&container, &["semantic", "skip", "label", "remote"], errors);
    let serde_c = serde_attrs::container(&input.attrs);
    let skipped = skip_reason(&container, errors);
    let remote = match assigned(&container, "remote") {
        Some(r) => match r.as_type() {
            Some(t) => Some(t.clone()),
            None => {
                errors.push(r.span(), "remote = path::To::Enum");
                return TokenStream::new();
            }
        },
        None => None,
    };
    let mut arms = Arms {
        options: Vec::new(),
        wires: Vec::new(),
        parses: Vec::new(),
        forms: Vec::new(),
        units: Vec::new(),
    };
    for v in &e.variants {
        variant_arms(
            name,
            v,
            skipped.is_some(),
            remote.as_ref(),
            serde_c.rename_all.as_deref(),
            &mut arms,
            errors,
        );
    }
    let all_unit = e.variants.iter().all(|v| matches!(v.fields, Fields::Unit));
    let forms = &arms.forms;
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
    if let Some(r) = &remote {
        if !all_unit || assigned(&container, "semantic").is_some() {
            errors.push(
                name.span(),
                "`remote` describes an enum of unit variants only",
            );
            return TokenStream::new();
        }
        return remote_impls(&p, r, &arms);
    }
    if let Some(sem_term) = assigned(&container, "semantic") {
        return semantic_impls(name, &p, sem_term, forms_fn, errors);
    }
    if !all_unit {
        errors.push(
            name.span(),
            format!("enum `{name}` has data in its variants; describe the whole enum with #[admin(semantic = ...)]"),
        );
        return TokenStream::new();
    }
    unit_impls(name, &p, &arms)
}

fn variant_arms(
    name: &syn::Ident,
    v: &syn::Variant,
    skipped: bool,
    remote: Option<&syn::Type>,
    rename_all: Option<&str>,
    arms: &mut Arms,
    errors: &mut Errors,
) {
    let p = p();
    let vident = &v.ident;
    let terms = if skipped {
        admin_attrs(&v.attrs, errors).unwrap_or_default()
    } else {
        match admin_attrs(&v.attrs, errors) {
            Some(t) => t,
            None => {
                errors.push(
                    v.span(),
                    format!("variant `{vident}` of `{name}` needs #[admin]"),
                );
                return;
            }
        }
    };
    check_keys(&terms, VARIANT_KEYS, errors);
    let wire = serde_attrs::field(&v.attrs)
        .rename
        .unwrap_or_else(|| serde_attrs::rename_variant(&vident.to_string(), rename_all));
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
        arms.options.push(quote!(#p::EnumOption { value: #wire.into(), label: #label.into(), help: #help.into(), danger: #danger }));
        let target = match remote {
            Some(r) => quote!(#r::#vident),
            None => quote!(#name::#vident),
        };
        arms.wires.push(quote!(#target => #wire));
        arms.parses
            .push(quote!(#wire => ::core::option::Option::Some(#target)));
        arms.units.push(quote!(#name::#vident));
    }
    arms.forms.push(quote!(#pat => #form));
}

fn remote_impls(p: &TokenStream, r: &syn::Type, arms: &Arms) -> TokenStream {
    let (options, wires, parses, units) = (&arms.options, &arms.wires, &arms.parses, &arms.units);
    quote! {
        impl #p::AdminEnum for #r {
            fn options() -> ::std::vec::Vec<#p::EnumOption> {
                ::std::vec![#(#options),*]
            }
            fn wire(&self) -> &'static str {
                match self {
                    #(#wires),*
                }
            }
            fn from_wire(s: &str) -> ::core::option::Option<Self> {
                match s {
                    #(#parses,)*
                    _ => ::core::option::Option::None,
                }
            }
        }
        impl #p::AdminSchema for #r {
            fn node() -> #p::Node {
                #p::Node::new(#p::SemType::Enum(#p::EnumC { options: <#r as #p::AdminEnum>::options() }))
            }
        }
        impl #p::Carrier<#p::sem::Enum> for #r {}
        impl #p::DefaultSem for #r {
            const SEM: &'static str = "enum";
            fn default_node() -> #p::Node {
                <#r as #p::AdminSchema>::node()
            }
        }
        const _: () = {
            let _ = [#(#units),*];
        };
    }
}

fn semantic_impls(
    name: &syn::Ident,
    p: &TokenStream,
    sem_term: &Term,
    forms_fn: TokenStream,
    errors: &mut Errors,
) -> TokenStream {
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
    let objects: Vec<&syn::Type> = sem_term
        .args()
        .iter()
        .filter(|a| a.name() == Some("object"))
        .filter_map(|a| a.args().first().and_then(Term::as_type))
        .collect();
    let nest = match objects.as_slice() {
        [one] => quote! {
            impl ::burrow_config::field::Nest for #name {
                type Inner = #one;
            }
        },
        _ => quote!(),
    };
    quote! {
        #nest
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
    }
}

fn unit_impls(name: &syn::Ident, p: &TokenStream, arms: &Arms) -> TokenStream {
    let (options, wires, parses, units) = (&arms.options, &arms.wires, &arms.parses, &arms.units);
    quote! {
        impl #name {
            pub const ALL: &'static [#name] = &[#(#units),*];
            pub const fn wire(&self) -> &'static str {
                match self {
                    #(#wires),*
                }
            }
            pub fn from_wire(s: &str) -> ::core::option::Option<Self> {
                match s {
                    #(#parses,)*
                    _ => ::core::option::Option::None,
                }
            }
        }
        impl #p::AdminEnum for #name {
            fn options() -> ::std::vec::Vec<#p::EnumOption> {
                ::std::vec![#(#options),*]
            }
            fn wire(&self) -> &'static str {
                #name::wire(self)
            }
            fn from_wire(s: &str) -> ::core::option::Option<Self> {
                #name::from_wire(s)
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
