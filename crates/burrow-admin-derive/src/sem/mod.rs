mod infer;
mod relation;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};

use crate::Errors;
use crate::term::Term;

pub(crate) use infer::{duration_unit, infer, primitive_int, reject_redundant, tag_of};
pub(crate) use relation::relations;

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
        "host_name" => "HostName",
        "server_address" => "ServerAddress",
        "cidr" => "Cidr",
        "cidr_rule" => "CidrRule",
        "country" => "Country",
        "duration" => "Duration",
        "size" => "Size",
        "rate" => "Rate",
        "ref_" => "Ref",
        "ref_list" => "RefList",
        "ref_entries" => "RefEntries",
        "ref_map" => "RefMap",
        "name" => "Name",
        "secret_ref" => "SecretRef",
        "path" => "Path",
        "upload" => "Upload",
        "password_hash" => "PasswordHash",
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

fn reserved(args: &[Term]) -> TokenStream {
    let names: Vec<&str> = Term::value(args, "reserved")
        .and_then(Term::as_str)
        .into_iter()
        .collect();
    quote!(::std::vec![#(::std::string::String::from(#names)),*])
}

fn entity(t: Option<&Term>, span: Span, errors: &mut Errors) -> TokenStream {
    let p = crate::p();
    match t.and_then(Term::as_word) {
        Some(w @ ("client" | "visitor" | "service" | "login" | "oidc")) => {
            quote!(#p::Entity::parse(#w))
        }
        _ => {
            errors.push(
                span,
                "expected an entity: client, visitor, service, login or oidc",
            );
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
    let sem = match name {
        "object" | "map" | "ref_map" => composite_sem(name, args, ty, span, &mut checks, errors),
        "list" => list_sem(args, ty, &m, span, &mut checks, errors),
        "union" | "union_" => union_sem(args, ty, &m, span, &mut checks, errors),
        _ => Sem::Type(scalar_sem(name, args, ty, range, span, errors)),
    };
    let sem = match sem {
        Sem::Type(sem) => sem,
        Sem::Node(node) => return Built { node, checks },
    };
    Built {
        node: quote!(#p::Node::new(#sem)),
        checks,
    }
}

enum Sem {
    Type(TokenStream),
    Node(TokenStream),
}

fn scalar_sem(
    name: &str,
    args: &[Term],
    ty: Option<&syn::Type>,
    range: (Option<i64>, Option<i64>),
    span: Span,
    errors: &mut Errors,
) -> TokenStream {
    let p = crate::p();
    let (min, max) = range;
    match name {
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
        "enum_" => match Term::positional(args)
            .first()
            .and_then(|a| match a {
                Term::Type(t, _) => Some((**t).clone()),
                _ => None,
            })
            .as_ref()
            .or(ty)
        {
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
        "host_name" => quote!(#p::SemType::HostName(#p::HostNameC::default())),
        "server_address" => quote!(#p::SemType::ServerAddress),
        "cidr" => quote!(#p::SemType::Cidr),
        "cidr_rule" => quote!(#p::SemType::CidrRule),
        "country" => quote!(#p::SemType::Country(#p::CountryC::default())),
        "rate" => quote!(#p::SemType::Rate),
        "url_path" => quote!(#p::SemType::UrlPath),
        "password_hash" => quote!(#p::SemType::PasswordHash(#p::PasswordHashC::default())),
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
            let reserved = reserved(args);
            quote!(#p::SemType::Ref(#p::RefC { entity: #e, reserved: #reserved }))
        }
        "ref_list" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            let ordered = Term::flag(args, "ordered");
            let max = opt_int(Term::value(args, "max").and_then(Term::as_int));
            let reserved = reserved(args);
            quote!(#p::SemType::RefList(#p::RefListC { entity: #e, ordered: #ordered, unique: true, max: #max, reserved: #reserved }))
        }
        "ref_entries" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            let reserved = reserved(args);
            quote!(#p::SemType::RefEntries(#p::RefEntriesC::new(#e, #reserved)))
        }
        "name" => {
            let e = entity(Term::positional(args).first().copied(), span, errors);
            quote!(#p::SemType::Name(#p::NameC { entity: #e, max_len: 64 }))
        }
        "secret_ref" => {
            let max = Term::value(args, "max").and_then(Term::as_int).unwrap_or(4) as u32;
            let reference = Term::flag(args, "reference");
            quote!(#p::SemType::SecretRef(#p::SecretRefC { max: #max, reference: #reference }))
        }
        "path" => {
            let must = Term::flag(args, "must_exist");
            let rel = opt_str(Term::value(args, "relative_to").and_then(Term::as_str));
            quote!(#p::SemType::Path(#p::PathC { must_exist: #must, relative_to: #rel }))
        }
        "upload" => {
            let kind = match Term::positional(args).first().and_then(|t| t.as_word()) {
                Some(w @ ("pair" | "ca" | "crl")) => w,
                _ => {
                    errors.push(span, "expected an upload kind: pair, ca or crl");
                    "pair"
                }
            };
            quote!(#p::SemType::Upload(#p::UploadC::new(#kind)))
        }
        _ => unreachable!("marker() covers every name"),
    }
}

fn composite_sem(
    name: &str,
    args: &[Term],
    ty: Option<&syn::Type>,
    span: Span,
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) -> Sem {
    let p = crate::p();
    Sem::Type(match name {
        "object" => {
            let explicit = Term::positional(args).first().and_then(|a| match a {
                Term::Type(t, _) => Some((**t).clone()),
                _ => None,
            });
            match explicit.as_ref().or(ty) {
                Some(ty) => {
                    let ty = ty.clone();
                    return Sem::Node(quote!(<#ty as #p::AdminSchema>::node()));
                }
                None => {
                    errors.push(span, "`object` needs a type here: object(TypeName)");
                    quote!(#p::SemType::Bool)
                }
            }
        }
        "map" => {
            let key = match Term::value(args, "key") {
                Some(k) => {
                    let b = build(k, None, (None, None), errors);
                    checks.extend(b.checks);
                    b.node
                }
                None => {
                    let e = entity(Term::positional(args).first().copied(), span, errors);
                    quote!(#p::Node::new(#p::SemType::Name(#p::NameC { entity: #e, max_len: 64 })))
                }
            };
            let value = match Term::value(args, "value") {
                Some(v) => {
                    let b = build(v, None, (None, None), errors);
                    checks.extend(b.checks);
                    Some(b.node)
                }
                None => ty
                    .and_then(|t| strip(t, "BTreeMap"))
                    .map(|v| quote!(<#v as #p::AdminSchema>::node())),
            };
            let value = match (
                value,
                Term::value(args, "value_help").and_then(Term::as_str),
            ) {
                (Some(v), Some(h)) => Some(quote!({ let mut n = #v; n.help = #h.into(); n })),
                (v, _) => v,
            };
            match value {
                Some(value) => quote!(#p::SemType::Map(#p::MapC {
                    key: ::std::boxed::Box::new(#key),
                    value: ::std::boxed::Box::new(#value),
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
            let reserved = reserved(args);
            quote!(#p::SemType::RefMap(#p::RefMapC { entity: #e, value: ::std::boxed::Box::new(#value), reserved: #reserved }))
        }
        _ => unreachable!("composite_sem covers object, map and ref_map"),
    })
}

fn list_sem(
    args: &[Term],
    ty: Option<&syn::Type>,
    m: &syn::Ident,
    span: Span,
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) -> Sem {
    let p = crate::p();
    let item_ty = ty.and_then(|t| strip(t, "Vec"));
    if let Some(ty) = ty {
        checks.push(check(ty, m, span));
    }
    let Some(item) = Term::positional(args).first().copied() else {
        errors.push(span, "`list` needs an item type: list(cidr)");
        return Sem::Node(quote!(#p::Node::default()));
    };
    let b = build(item, item_ty, (None, None), errors);
    checks.extend(b.checks);
    let max_items = opt_int(Term::value(args, "max").and_then(Term::as_int));
    let item_node = b.node;
    Sem::Type(
        quote!(#p::SemType::List(#p::ListC { item: ::std::boxed::Box::new(#item_node), max_items: #max_items })),
    )
}

fn union_sem(
    args: &[Term],
    ty: Option<&syn::Type>,
    m: &syn::Ident,
    span: Span,
    checks: &mut Vec<TokenStream>,
    errors: &mut Errors,
) -> Sem {
    let p = crate::p();
    let branches: Vec<&Term> = Term::positional(args);
    Sem::Type(if branches.is_empty() {
        match ty {
            Some(ty) => {
                checks.push(check(ty, m, span));
                return Sem::Node(quote!(<#ty as #p::AdminSchema>::node()));
            }
            None => {
                errors.push(span, "`union` needs branches: union(bool, object(T))");
                quote!(#p::SemType::Bool)
            }
        }
    } else {
        if let Some(ty) = ty {
            checks.push(check(ty, m, span));
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
    })
}
