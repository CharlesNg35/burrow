use super::tree::{FromEnv, Node, Pos, Span, Value};
use crate::diagnostics::Diagnostic;

pub trait EnvSource {
    fn get(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl<F: Fn(&str) -> Option<String>> EnvSource for F {
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

pub fn expand_value(s: &str, env: &dyn EnvSource) -> Option<String> {
    expand_str(s, Span::default(), "", env, false, &mut Vec::new())
}

pub fn expand(node: &mut Node, src: &str, env: &dyn EnvSource, out: &mut Vec<Diagnostic>) {
    expand_in(node, "", src, env, out);
}

fn expand_in(
    node: &mut Node,
    key: &str,
    src: &str,
    env: &dyn EnvSource,
    out: &mut Vec<Diagnostic>,
) {
    match &mut node.value {
        Value::Str(s) if s.contains('$') => {
            let span = node.span;
            let errors = out.len();
            let referenced = s.replace("$${", "").contains("${");
            match expand_str(s, span, src, env, key == "secret", out) {
                Some(v) => *s = v,
                None if out.len() > errors => node.env = FromEnv::Unset,
                None => {}
            }
            if referenced && node.env == FromEnv::No {
                node.env = FromEnv::Expanded;
            }
        }
        Value::Seq(items) => items
            .iter_mut()
            .for_each(|n| expand_in(n, key, src, env, out)),
        Value::Map(entries) => entries
            .iter_mut()
            .for_each(|e| expand_in(&mut e.value, &e.key, src, env, out)),
        _ => {}
    }
}

fn expand_str(
    s: &str,
    span: Span,
    src: &str,
    env: &dyn EnvSource,
    secret: bool,
    out: &mut Vec<Diagnostic>,
) -> Option<String> {
    use burrow_syntax::env::Piece;
    let mut result = String::with_capacity(s.len());
    let mut ok = true;
    for piece in burrow_syntax::env::pieces(s) {
        match piece {
            Piece::Text(t) => result.push_str(t),
            Piece::Escape => result.push_str("${"),
            Piece::Unclosed(tail) => {
                out.push(
                    Diagnostic::error("env-syntax", "unclosed \"${\" in value")
                        .at(sub_span(src, span, tail))
                        .label("missing \"}\"")
                        .help("write ${NAME}, or $${ for a literal \"${\""),
                );
                return None;
            }
            Piece::Var { name, valid } => {
                let token = format!("${{{name}}}");
                if !valid {
                    out.push(
                        Diagnostic::error(
                            "env-syntax",
                            format!("invalid environment variable name \"{name}\""),
                        )
                        .at(sub_span(src, span, &token))
                        .label("not a valid variable name")
                        .help(
                            "names use letters, digits and underscores, and do not start with a digit",
                        ),
                    );
                    ok = false;
                } else {
                    match env.get(name) {
                        Some(v) => result.push_str(&v),
                        None => {
                            out.push(
                                Diagnostic::error(
                                    "env-unset",
                                    format!("environment variable \"{name}\" is not set"),
                                )
                                .at(sub_span(src, span, &token))
                                .label("referenced here")
                                .help(set_hint(name, secret)),
                            );
                            ok = false;
                        }
                    }
                }
            }
        }
    }
    ok.then_some(result)
}

fn set_hint(name: &str, secret: bool) -> String {
    let set = if cfg!(windows) {
        format!("$env:{name} = \"...\"")
    } else {
        format!("export {name}=\"...\"")
    };
    let hint = format!("set it before starting burrow, for example: {set}");
    if secret {
        format!(
            "{hint}; a secret has the same value in server.yaml and in the client's or visitor's file (make a new one with: {})",
            crate::base::failure::cmd("gensecret")
        )
    } else {
        hint
    }
}

fn sub_span(src: &str, span: Span, token: &str) -> Span {
    let start = span.start.offset.min(src.len());
    let end = span.end.offset.clamp(start, src.len());
    let Some(rel) = src.get(start..end).and_then(|t| t.find(token)) else {
        return span;
    };
    let before = &src[start..start + rel];
    if before.contains('\n') {
        return span;
    }
    let col = span.start.col + before.chars().count();
    let width = token.chars().count();
    Span {
        start: Pos {
            offset: start + rel,
            line: span.start.line,
            col,
        },
        end: Pos {
            offset: start + rel + token.len(),
            line: span.start.line,
            col: col + width,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tree::parse;

    fn env(name: &str) -> Option<String> {
        match name {
            "A" => Some("alpha".into()),
            _ => None,
        }
    }

    #[test]
    fn expands_and_reports() {
        let src = "x: pre-${A}-post\ny: ${MISSING}\nz: $${A} and $5\n";
        let mut n = parse(src).unwrap();
        let mut out = Vec::new();
        expand(&mut n, src, &env, &mut out);
        assert_eq!(n.get("x").unwrap().as_str(), Some("pre-alpha-post"));
        assert_eq!(n.get("z").unwrap().as_str(), Some("${A} and $5"));
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].message,
            "environment variable \"MISSING\" is not set"
        );
        let s = out[0].span.unwrap();
        assert_eq!((s.start.line, s.start.col, s.end.col), (2, 4, 14));
    }

    #[test]
    fn quoted_scalar_span() {
        let src = "{\"secret\": \"${MISSING}\"}";
        let mut n = parse(src).unwrap();
        let mut out = Vec::new();
        expand(&mut n, src, &env, &mut out);
        let s = out[0].span.unwrap();
        assert_eq!(&src[s.start.offset..s.end.offset], "${MISSING}");
        assert_eq!(s.start.col, 13);
    }

    #[test]
    fn expansion_and_the_scanners_agree_on_which_variables_a_value_uses() {
        for text in [
            "${A}",
            "$${A}",
            "$$${A}",
            "${A:-d}",
            "${1X}",
            "${_X1}",
            "${A${B}}",
            "x ${A",
            "$5 ${B}",
            "${A}-${B}-${A}",
            "pre$${A}${B}post",
            "${}",
        ] {
            let asked = std::cell::RefCell::new(Vec::new());
            let record = |name: &str| {
                if !asked.borrow().iter().any(|n: &String| n == name) {
                    asked.borrow_mut().push(name.to_string());
                }
                Some("v".to_string())
            };
            expand_str(text, Span::default(), "", &record, false, &mut Vec::new());
            let scanned = burrow_syntax::env::names(text);
            assert_eq!(*asked.borrow(), scanned, "{text}");
            let yaml = format!("x: '{text}'\n");
            assert_eq!(crate::service::referenced_env(&yaml), scanned, "{text}");
        }
    }
}
