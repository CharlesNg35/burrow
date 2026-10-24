use proc_macro2::TokenStream;
use quote::quote;

use crate::Errors;
use crate::term::Term;

use super::entity;

fn cond(t: &Term, fields: &[String], owner: &syn::Ident, errors: &mut Errors) -> TokenStream {
    let p = crate::p();
    let field_ref = |a: Option<&Term>, errors: &mut Errors| -> String {
        match a {
            Some(Term::Word(w, s)) => {
                if !fields.iter().any(|f| f == w) {
                    errors.push(*s, format!("`{w}` is not a field of `{owner}`"));
                }
                w.clone()
            }
            Some(Term::Str(s, _)) if s.starts_with('/') => s.clone(),
            Some(other) => {
                errors.push(
                    other.span(),
                    "expected a field name or an absolute \"/path\"",
                );
                String::new()
            }
            None => {
                errors.push(t.span(), "missing field name");
                String::new()
            }
        }
    };
    let lit = |a: Option<&Term>| -> TokenStream {
        match a {
            Some(Term::Str(s, _)) => quote!(#p::json!(#s)),
            Some(Term::Int(i, _)) => quote!(#p::json!(#i)),
            Some(Term::Word(w, _)) if w == "true" || w == "false" => {
                let b = w == "true";
                quote!(#p::json!(#b))
            }
            _ => quote!(#p::json!(null)),
        }
    };
    let args = t.args();
    match t.name() {
        Some("eq") => {
            let f = field_ref(args.first(), errors);
            let v = lit(args.get(1));
            quote!(#p::Cond::Eq(#f.into(), #v))
        }
        Some("ne") => {
            let f = field_ref(args.first(), errors);
            let v = lit(args.get(1));
            quote!(#p::Cond::Ne(#f.into(), #v))
        }
        Some("len_ge") => {
            let f = field_ref(args.first(), errors);
            let n = args.get(1).and_then(Term::as_int).unwrap_or(1) as u64;
            quote!(#p::Cond::LenGe(#f.into(), #n))
        }
        Some("set") => {
            let f = field_ref(args.first(), errors);
            quote!(#p::Cond::Set(#f.into()))
        }
        Some("all") | Some("any") => {
            let parts: Vec<TokenStream> = args
                .iter()
                .map(|a| cond(a, fields, owner, errors))
                .collect();
            if t.name() == Some("all") {
                quote!(#p::Cond::All(::std::vec![#(#parts),*]))
            } else {
                quote!(#p::Cond::Any(::std::vec![#(#parts),*]))
            }
        }
        Some("not") => {
            let inner = match args.first() {
                Some(a) => cond(a, fields, owner, errors),
                None => {
                    errors.push(t.span(), "not(...) needs a condition");
                    quote!(#p::Cond::Set(::std::string::String::new()))
                }
            };
            quote!(#p::Cond::Not(::std::boxed::Box::new(#inner)))
        }
        _ => {
            errors.push(
                t.span(),
                "expected a condition: eq, ne, len_ge, set, all, any or not",
            );
            quote!(#p::Cond::Set(::std::string::String::new()))
        }
    }
}

pub fn relations(
    terms: &[Term],
    fields: &[String],
    owner: &syn::Ident,
    current: &str,
    errors: &mut Errors,
) -> Vec<TokenStream> {
    let p = crate::p();
    let mut out = Vec::new();
    let names = |args: &[Term], errors: &mut Errors| -> Vec<String> {
        args.iter()
            .filter_map(|a| match a {
                Term::Word(w, s) => {
                    if !fields.iter().any(|f| f == w) {
                        errors.push(*s, format!("`{w}` is not a field of `{owner}`"));
                    }
                    Some(w.clone())
                }
                Term::Str(s, _) if s.starts_with('/') => Some(s.clone()),
                other => {
                    errors.push(other.span(), "expected a field name");
                    None
                }
            })
            .collect()
    };
    for t in terms {
        let args = t.args();
        match t.name() {
            Some("visible_if") => match args.first() {
                Some(c) => {
                    let c = cond(c, fields, owner, errors);
                    out.push(quote!(#p::Relation::VisibleIf(#c)));
                }
                None => errors.push(t.span(), "visible_if(condition)"),
            },
            Some("enabled_if") => match (args.first(), args.get(1).and_then(Term::as_str)) {
                (Some(c), Some(reason)) => {
                    let c = cond(c, fields, owner, errors);
                    out.push(quote!(#p::Relation::EnabledIf { cond: #c, reason: #reason.into() }));
                }
                _ => errors.push(t.span(), "enabled_if(condition, \"reason\")"),
            },
            Some("options_filter") => match args.first() {
                Some(c) => {
                    let c = cond(c, &[String::from("protocol")], owner, errors);
                    out.push(quote!(#p::Relation::OptionsFilter(#c)));
                }
                None => errors.push(t.span(), "options_filter(condition)"),
            },
            Some("one_of") => match t {
                Term::Assign(_, v, _) if v.as_str().is_some() => {
                    let g = v.as_str().unwrap_or_default();
                    out.push(quote!(#p::Relation::OneOf { group: #g.into() }));
                }
                _ => errors.push(t.span(), "one_of = \"group\""),
            },
            Some("inverse") => {
                let e = entity(Term::value(args, "entity"), t.span(), errors);
                let label = Term::value(args, "label")
                    .and_then(Term::as_str)
                    .unwrap_or("");
                out.push(quote!(#p::Relation::Inverse { entity: #e, field: #current.into(), label: #label.into() }));
            }
            Some("default_from") => match args.first().and_then(Term::as_str) {
                Some(path) => out.push(quote!(#p::Relation::DefaultFrom(#path.into()))),
                None => errors.push(t.span(), "default_from(\"/absolute/path\")"),
            },
            Some("cross") => {
                let ns = names(args, errors);
                out.push(quote!(#p::Relation::Cross(::std::vec![#(#ns.into()),*])));
            }
            Some("same_len") => {
                let ns = names(args, errors);
                match ns.first() {
                    Some(n) => out.push(quote!(#p::Relation::SameLen(#n.into()))),
                    None => errors.push(t.span(), "same_len(field)"),
                }
            }
            Some("warn_if") => match (args.first(), args.get(1).and_then(Term::as_str)) {
                (Some(c), Some(msg)) => {
                    let c = cond(c, fields, owner, errors);
                    out.push(quote!(#p::Relation::WarnIf { cond: #c, message: #msg.into() }));
                }
                _ => errors.push(t.span(), "warn_if(condition, \"message\")"),
            },
            _ => {}
        }
    }
    out
}
