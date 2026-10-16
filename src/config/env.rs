use super::tree::{Node, Pos, Span, Value};
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

pub fn expand(node: &mut Node, src: &str, env: &dyn EnvSource, out: &mut Vec<Diagnostic>) {
    match &mut node.value {
        Value::Str(s) if s.contains('$') => {
            let span = node.span;
            if let Some(v) = expand_str(s, span, src, env, out) {
                *s = v;
            }
        }
        Value::Seq(items) => items.iter_mut().for_each(|n| expand(n, src, env, out)),
        Value::Map(entries) => entries
            .iter_mut()
            .for_each(|e| expand(&mut e.value, src, env, out)),
        _ => {}
    }
}

fn expand_str(
    s: &str,
    span: Span,
    src: &str,
    env: &dyn EnvSource,
    out: &mut Vec<Diagnostic>,
) -> Option<String> {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;
    let mut ok = true;
    while let Some(i) = rest.find('$') {
        result.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(t) = tail.strip_prefix("$${") {
            result.push_str("${");
            rest = t;
        } else if let Some(t) = tail.strip_prefix("${") {
            let Some(end) = t.find('}') else {
                out.push(
                    Diagnostic::error("env-syntax", "unclosed \"${\" in value")
                        .at(sub_span(src, span, tail))
                        .label("missing \"}\"")
                        .help("write ${NAME}, or $${ for a literal \"${\""),
                );
                return None;
            };
            let name = &t[..end];
            let token = &tail[..end + 3];
            if !valid_name(name) {
                out.push(
                    Diagnostic::error(
                        "env-syntax",
                        format!("invalid environment variable name \"{name}\""),
                    )
                    .at(sub_span(src, span, token))
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
                            .at(sub_span(src, span, token))
                            .label("referenced here")
                            .help(format!(
                                "set it before starting burrow, for example: export {name}=\"$(burrow gensecret)\" (for a secret) or export {name}=value"
                            )),
                        );
                        ok = false;
                    }
                }
            }
            rest = &t[end + 1..];
        } else {
            result.push('$');
            rest = &tail[1..];
        }
    }
    result.push_str(rest);
    ok.then_some(result)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
}
