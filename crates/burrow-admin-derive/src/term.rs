use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Expr, Lit};

#[derive(Debug, Clone)]
pub enum Term {
    Word(String, Span),
    Call(String, Vec<Term>, Span),
    Assign(String, Box<Term>, Span),
    Str(String, Span),
    Int(i64, Span),
    Type(syn::Type, Span),
}

impl Term {
    pub fn span(&self) -> Span {
        match self {
            Term::Word(_, s)
            | Term::Call(_, _, s)
            | Term::Assign(_, _, s)
            | Term::Str(_, s)
            | Term::Int(_, s)
            | Term::Type(_, s) => *s,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Term::Word(n, _) | Term::Call(n, _, _) | Term::Assign(n, _, _) => Some(n),
            _ => None,
        }
    }

    pub fn args(&self) -> &[Term] {
        match self {
            Term::Call(_, a, _) => a,
            _ => &[],
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Term::Str(s, _) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Term::Int(i, _) => Some(*i),
            _ => None,
        }
    }

    pub fn as_word(&self) -> Option<&str> {
        match self {
            Term::Word(w, _) => Some(w),
            _ => None,
        }
    }

    pub fn flag(args: &[Term], name: &str) -> bool {
        args.iter().any(|a| a.as_word() == Some(name))
    }

    pub fn value<'a>(args: &'a [Term], name: &str) -> Option<&'a Term> {
        args.iter().find_map(|a| match a {
            Term::Assign(n, v, _) if n == name => Some(&**v),
            _ => None,
        })
    }

    pub fn positional(args: &[Term]) -> Vec<&Term> {
        args.iter()
            .filter(|a| !matches!(a, Term::Assign(..)))
            .collect()
    }
}

fn ident_of(e: &Expr) -> Option<String> {
    match e {
        Expr::Path(p) if p.qself.is_none() => {
            let segs: Vec<String> = p
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            Some(segs.join("::"))
        }
        _ => None,
    }
}

pub fn parse(e: &Expr) -> syn::Result<Term> {
    let span = e.span();
    match e {
        Expr::Path(_) => {
            let name = ident_of(e).ok_or_else(|| syn::Error::new(span, "expected a name"))?;
            if name.contains("::") || name.chars().next().is_some_and(char::is_uppercase) {
                let ty: syn::Type = syn::parse_str(&name)?;
                return Ok(Term::Type(ty, span));
            }
            Ok(Term::Word(name, span))
        }
        Expr::Call(c) => {
            let name = ident_of(&c.func)
                .ok_or_else(|| syn::Error::new(c.func.span(), "expected a name before ( )"))?;
            let args = c.args.iter().map(parse).collect::<syn::Result<Vec<_>>>()?;
            Ok(Term::Call(name, args, span))
        }
        Expr::Assign(a) => {
            let name = ident_of(&a.left)
                .ok_or_else(|| syn::Error::new(a.left.span(), "expected a name before ="))?;
            Ok(Term::Assign(name, Box::new(parse(&a.right)?), span))
        }
        Expr::Lit(l) => match &l.lit {
            Lit::Str(s) => Ok(Term::Str(s.value(), span)),
            Lit::Int(i) => Ok(Term::Int(i.base10_parse()?, span)),
            Lit::Bool(b) => Ok(Term::Word(b.value.to_string(), span)),
            _ => Err(syn::Error::new(span, "expected a string or an integer")),
        },
        Expr::Unary(u) if matches!(u.op, syn::UnOp::Neg(_)) => match parse(&u.expr)? {
            Term::Int(i, s) => Ok(Term::Int(-i, s)),
            _ => Err(syn::Error::new(span, "expected a number")),
        },
        Expr::Group(g) => parse(&g.expr),
        Expr::Paren(p) => parse(&p.expr),
        _ => Err(syn::Error::new(
            span,
            "unsupported expression in #[admin(...)]",
        )),
    }
}

pub fn parse_attr(attr: &syn::Attribute) -> syn::Result<Vec<Term>> {
    let list = attr.meta.require_list()?;
    let exprs = list
        .parse_args_with(syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated)?;
    exprs.iter().map(parse).collect()
}
