use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};

use crate::Errors;
use crate::term::Term;

pub struct Built {
    pub node: TokenStream,
    pub checks: Vec<TokenStream>,
}

pub fn strip<'a>(ty: &'a syn::Type, wrapper: &str) -> Option<&'a syn::Type> {
    let syn::Type::Path(tp) = ty else {
        return None;
    };
    let last = tp.path.segments.last()?;
    if last.ident != wrapper {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().rev().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

pub fn marker(t: &Term) -> Option<syn::Ident> {
    let name = t.name()?;
    let m = match name {
        "text" => "Text",
        "integer" => "Integer",
        "bool" => "Bool",
        "enum_" | "enum_values" => "Enum",
        "ip" => "Ip",
        "port" => "Port",
        "ip_port" => "IpPort",
        "port_range" => "PortRange",
        "target" => "Target",
        "host" => "Host",
        "server_address" => "ServerAddress",
        "cidr" => "Cidr",
        "cidr_rule" => "CidrRule",
        "duration" => "Duration",
        "size" => "Size",
        "rate" => "Rate",
        "ref_" => "Ref",
        "ref_list" => "RefList",
        "ref_map" => "RefMap",
        "name" => "Name",
        "secret_ref" => "SecretRef",
        "path" => "Path",
        "url_path" => "UrlPath",
        "object" => "Object",
        "map" => "Map",
        "list" => "List",
        "union" | "union_" => "Union",
        _ => return None,
    };
    Some(format_ident!("{}", m))
}

fn opt_int(v: Option<i64>) -> TokenStream {
    match v {
        Some(i) => quote!(::core::option::Option::Some(#i as _)),
        None => quote!(::core::option::Option::None),
    }
}

fn opt_str(v: Option<&str>) -> TokenStream {
    match v {
        Some(s) => quote!(::core::option::Option::Some(#s.into())),
        None => quote!(::core::option::Option::None),
    }
}

fn entity(t: Option<&Term>, span: Span, errors: &mut Errors) -> TokenStream {
    let p = crate::p();
    match t.and_then(Term::as_word) {
        Some(w @ ("client" | "visitor" | "service")) => quote!(#p::Entity::parse(#w)),
        _ => {
            errors.push(span, "expected an entity: client, visitor or service");
            quote!(#p::Entity::Client)
        }
    }
}

fn check(ty: &syn::Type, marker: &syn::Ident, span: Span) -> TokenStream {
    let p = crate::p();
    quote_spanned!(span=> #p::assert_carrier::<#ty, #p::sem::#marker>();)
}

pub fn build(
    t: &Term,
    ty: Option<&syn::Type>,
    range: (Option<i64>, Option<i64>),
    errors: &mut Errors,
) -> Built {
    let p = crate::p();
    let span = t.span();
    let mut checks = Vec::new();
    let Some(name) = t.name() else {
        errors.push(span, "expected a semantic type, like ty = ip_port");
        return Built {
            node: quote!(#p::Node::default()),
            checks,
        };
    };
    let args = t.args();
    let Some(m) = marker(t) else {
        errors.push(span, format!("unknown semantic type `{name}`"));
        return Built {
            node: quote!(#p::Node::default()),
            checks,
        };
    };
    if let Some(ty) = ty
        && !matches!(name, "list" | "union")
    {
        checks.push(check(ty, &m, span));
    }
    let (min, max) = range;
    let sem = match name {
        "text" => {
            let max_len = opt_int(Term::value(args, "max_len").and_then(Term::as_int));
            let hint = opt_str(Term::value(args, "pattern_hint").and_then(Term::as_str));
            quote!(#p::SemType::Text(#p::TextC { max_len: #max_len, pattern_hint: #hint }))
        }
        "integer" => {
            let unit = opt_str(Term::value(args, "unit").and_then(Term::as_str));
            let (mn, mx) = (opt_int(min), opt_int(max));
            quote!(#p::SemType::Integer(#p::IntegerC { min: #mn, max: #mx, unit: #unit }))
        }
        "bool" => quote!(#p::SemType::Bool),
        "enum_" => match ty {
            Some(ty) => {
                quote!(#p::SemType::Enum(#p::EnumC { options: <#ty as #p::AdminEnum>::options() }))
            }
            None => {
                errors.push(
                    span,
                    "`enum_` needs a field type; use enum_values(\"a\", \"b\") here",
                );
                quote!(#p::SemType::Bool)
            }
        },
        "enum_values" => {
            let vals: Vec<&str> = args.iter().filter_map(Term::as_str).collect();
            quote!(#p::SemType::Enum(#p::EnumC { options: ::std::vec![#(#p::EnumOption { value: #vals.into(), label: #vals.into(), help: ::std::string::String::new(), danger: false }),*] }))
        }
        "ip" => quote!(#p::SemType::Ip(#p::IpC { v4: true, v6: true, unspecified_ok: true })),
        "port" => quote!(#p::SemType::Port),
        "ip_port" => {
            let ranges = Term::flag(args, "ranges");
            let max_range = opt_int(Term::value(args, "max_range").and_then(Term::as_int));
            let default_ip = opt_str(Some(
                Term::value(args, "default_ip")
                    .and_then(Term::as_str)
                    .unwrap_or("0.0.0.0"),
            ));
            quote!(#p::SemType::IpPort(#p::IpPortC { ranges: #ranges, max_range: #max_range, default_ip: #default_ip }))
        }
        "port_range" => {
            let max = opt_int(Term::value(args, "max").and_then(Term::as_int));
            quote!(#p::SemType::PortRange(#p::PortRangeC { max: #max }))
        }
        "target" => {
            let ranges = Term::flag(args, "ranges");
            let unix = Term::flag(args, "unix");
            quote!(#p::SemType::Target(#p::TargetC { ranges: #ranges, unix: #unix }))
        }
        "host" => quote!(#p::SemType::Host),
        "server_address" => quote!(#p::SemType::ServerAddress),
        "cidr" => quote!(#p::SemType::Cidr),
        "cidr_rule" => quote!(#p::SemType::CidrRule),
        "rate" => quote!(#p::SemType::Rate),
        "url_path" => quote!(#p::SemType::UrlPath),
        "duration" => {
            let unit = match Term::positional(args).first().and_then(|a| a.as_word()) {
                Some("ms") => quote!(#p::DurationUnit::Ms),
                Some("secs") | None => quote!(#p::DurationUnit::Secs),
                Some(_) => {
                    errors.push(span, "duration(secs) or duration(ms)");
                    quote!(#p::DurationUnit::Secs)
                }
            };
            let (mn, mx) = (opt_int(min), opt_int(max));
            quote!(#p::SemType::Duration(#p::DurationC { unit: #unit, min: #mn, max: #mx }))
        }
        "size" => {
            let unit = Term::value(args, "unit")
                .and_then(Term::as_str)
                .unwrap_or("bytes");
            quote!(#p::SemType::Size(#p::SizeC { unit: #unit.into() }))
        }
        "ref_" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            quote!(#p::SemType::Ref(#p::RefC { entity: #e }))
        }
        "ref_list" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            let ordered = Term::flag(args, "ordered");
            let max = opt_int(Term::value(args, "max").and_then(Term::as_int));
            quote!(#p::SemType::RefList(#p::RefListC { entity: #e, ordered: #ordered, unique: true, max: #max }))
        }
        "name" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            quote!(#p::SemType::Name(#p::NameC { entity: #e, max_len: 64 }))
        }
        "secret_ref" => {
            let max = Term::value(args, "max").and_then(Term::as_int).unwrap_or(4) as u32;
            quote!(#p::SemType::SecretRef(#p::SecretRefC { max: #max }))
        }
        "path" => {
            let must = Term::flag(args, "must_exist");
            let rel = opt_str(Term::value(args, "relative_to").and_then(Term::as_str));
            quote!(#p::SemType::Path(#p::PathC { must_exist: #must, relative_to: #rel }))
        }
        "object" => {
            let explicit = Term::positional(args).first().and_then(|a| match a {
                Term::Type(t, _) => Some(t.clone()),
                _ => None,
            });
            match explicit.as_ref().or(ty) {
                Some(ty) => {
                    let ty = ty.clone();
                    return Built {
                        node: quote!(<#ty as #p::AdminSchema>::node()),
                        checks,
                    };
                }
                None => {
                    errors.push(span, "`object` needs a type here: object(TypeName)");
                    quote!(#p::SemType::Bool)
                }
            }
        }
        "map" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            let value_ty = ty.and_then(|t| strip(t, "BTreeMap")).cloned();
            match value_ty {
                Some(v) => quote!(#p::SemType::Map(#p::MapC {
                    key: ::std::boxed::Box::new(#p::Node::new(#p::SemType::Name(#p::NameC { entity: #e, max_len: 64 }))),
                    value: ::std::boxed::Box::new(<#v as #p::AdminSchema>::node()),
                })),
                None => {
                    errors.push(span, "`map` needs a BTreeMap<String, T> field");
                    quote!(#p::SemType::Bool)
                }
            }
        }
        "ref_map" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            let value = match Term::value(args, "value") {
                Some(v) => {
                    let b = build(v, None, (None, None), errors);
                    checks.extend(b.checks);
                    b.node
                }
                None => match ty.and_then(|t| strip(t, "BTreeMap")) {
                    Some(v) => quote!(<#v as #p::AdminSchema>::node()),
                    None => {
                        errors.push(
                            span,
                            "`ref_map` needs value = ... or a BTreeMap<String, T> field",
                        );
                        quote!(#p::Node::default())
                    }
                },
            };
            quote!(#p::SemType::RefMap(#p::RefMapC { entity: #e, value: ::std::boxed::Box::new(#value) }))
        }
        "list" => {
            let item_ty = ty.and_then(|t| strip(t, "Vec"));
            if let Some(ty) = ty {
                checks.push(check(ty, &m, span));
            }
            let Some(item) = Term::positional(args).first().copied() else {
                errors.push(span, "`list` needs an item type: list(cidr)");
                return Built {
                    node: quote!(#p::Node::default()),
                    checks,
                };
            };
            let b = build(item, item_ty, (None, None), errors);
            checks.extend(b.checks);
            let max_items = opt_int(Term::value(args, "max").and_then(Term::as_int));
            let item_node = b.node;
            quote!(#p::SemType::List(#p::ListC { item: ::std::boxed::Box::new(#item_node), max_items: #max_items }))
        }
        "union" | "union_" => {
            let branches: Vec<&Term> = Term::positional(args);
            if branches.is_empty() {
                match ty {
                    Some(ty) => {
                        checks.push(check(ty, &m, span));
                        return Built {
                            node: quote!(<#ty as #p::AdminSchema>::node()),
                            checks,
                        };
                    }
                    None => {
                        errors.push(span, "`union` needs branches: union(bool, object(T))");
                        quote!(#p::SemType::Bool)
                    }
                }
            } else {
                if let Some(ty) = ty {
                    checks.push(check(ty, &m, span));
                }
                let nodes: Vec<TokenStream> = branches
                    .iter()
                    .map(|b| {
                        let built = build(b, None, (None, None), errors);
                        checks.extend(built.checks);
                        built.node
                    })
                    .collect();
                quote!(#p::SemType::Union(#p::UnionC { branches: ::std::vec![#(#nodes),*] }))
            }
        }
        _ => unreachable!("marker() covers every name"),
    };
    Built {
        node: quote!(#p::Node::new(#sem)),
        checks,
    }
}

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
