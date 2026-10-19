use syn::{Attribute, Expr, Lit, Meta};

#[derive(Default)]
pub struct Container {
    pub default: bool,
    pub rename_all: Option<String>,
}

pub enum Default {
    Plain,
    Path(String),
}

#[derive(std::default::Default)]
pub struct Field {
    pub rename: Option<String>,
    pub default: Option<Default>,
}

#[derive(std::default::Default)]
pub struct Schemars {
    pub description: Option<String>,
    pub min: Option<i64>,
    pub max: Option<i64>,
}

fn lit_str(e: &Expr) -> Option<String> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

fn lit_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse().ok(),
            _ => None,
        },
        Expr::Unary(u) => lit_int(&u.expr).map(|v| -v),
        _ => None,
    }
}

fn items(attr: &Attribute) -> Vec<Meta> {
    match &attr.meta {
        Meta::List(l) => l
            .parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)
            .map(|p| p.into_iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

pub fn container(attrs: &[Attribute]) -> Container {
    let mut c = Container::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        for m in items(a) {
            match &m {
                Meta::Path(p) if p.is_ident("default") => c.default = true,
                Meta::NameValue(nv) if nv.path.is_ident("rename_all") => {
                    c.rename_all = lit_str(&nv.value)
                }
                _ => {}
            }
        }
    }
    c
}

pub fn field(attrs: &[Attribute]) -> Field {
    let mut f = Field::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        for m in items(a) {
            match &m {
                Meta::Path(p) if p.is_ident("default") => f.default = Some(Default::Plain),
                Meta::NameValue(nv) if nv.path.is_ident("default") => {
                    f.default = lit_str(&nv.value).map(Default::Path)
                }
                Meta::NameValue(nv) if nv.path.is_ident("rename") => f.rename = lit_str(&nv.value),
                _ => {}
            }
        }
    }
    f
}

pub fn schemars(attrs: &[Attribute]) -> Schemars {
    let mut s = Schemars::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("schemars")) {
        for m in items(a) {
            match &m {
                Meta::NameValue(nv) if nv.path.is_ident("description") => {
                    s.description = lit_str(&nv.value)
                }
                Meta::List(l) if l.path.is_ident("range") => {
                    let inner = l
                        .parse_args_with(
                            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
                        )
                        .map(|p| p.into_iter().collect::<Vec<_>>())
                        .unwrap_or_default();
                    for i in inner {
                        if let Meta::NameValue(nv) = i {
                            if nv.path.is_ident("min") {
                                s.min = lit_int(&nv.value);
                            } else if nv.path.is_ident("max") {
                                s.max = lit_int(&nv.value);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    s
}

pub fn rename_variant(name: &str, rule: Option<&str>) -> String {
    let words = || {
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                out.push(' ');
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    };
    match rule {
        Some("lowercase") => name.to_ascii_lowercase(),
        Some("UPPERCASE") => name.to_ascii_uppercase(),
        Some("kebab-case") => words().replace(' ', "-"),
        Some("snake_case") => words().replace(' ', "_"),
        _ => name.to_string(),
    }
}
