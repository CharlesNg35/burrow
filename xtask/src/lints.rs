use std::path::Path;

use crate::shape::lex;

pub const SILENCED: &[&str] = &["too_many_lines", "pedantic", "all"];

#[derive(Debug, Clone, PartialEq)]
struct Tok {
    text: String,
    line: usize,
}

fn tokens(code: &str) -> Vec<Tok> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1);
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
        } else if c.is_whitespace() {
            i += 1;
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok {
                text: chars[start..i].iter().collect(),
                line,
            });
        } else if c == ':' && chars.get(i + 1) == Some(&':') {
            out.push(Tok {
                text: "::".into(),
                line,
            });
            i += 2;
        } else {
            out.push(Tok {
                text: c.to_string(),
                line,
            });
            i += 1;
        }
    }
    out
}

fn closing(toks: &[Tok], open: usize) -> usize {
    let (o, c) = match toks[open].text.as_str() {
        "(" => ("(", ")"),
        "[" => ("[", "]"),
        _ => ("{", "}"),
    };
    let mut depth = 0;
    for (k, t) in toks.iter().enumerate().skip(open) {
        if t.text == o {
            depth += 1;
        } else if t.text == c {
            depth -= 1;
            if depth == 0 {
                return k;
            }
        }
    }
    toks.len()
}

fn texts(toks: &[Tok]) -> Vec<&str> {
    toks.iter().map(|t| t.text.as_str()).collect()
}

pub fn silenced_problems(path: &str, src: &str) -> Vec<String> {
    let toks = tokens(&lex(src).code);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let at = if toks[i].text == "#" && toks.get(i + 1).is_some_and(|t| t.text == "!") {
            i + 2
        } else {
            i + 1
        };
        if toks[i].text != "#" || toks.get(at).is_none_or(|t| t.text != "[") {
            i += 1;
            continue;
        }
        let end = closing(&toks, at);
        let attr = &toks[at..end.min(toks.len())];
        for (k, t) in attr.iter().enumerate() {
            let lowers = ["allow", "expect", "warn"].contains(&t.text.as_str());
            if !lowers || attr.get(k + 1).is_none_or(|n| n.text != "(") {
                continue;
            }
            let inner = &attr[k + 1..closing(attr, k + 1).min(attr.len())];
            let words = texts(inner);
            let named = words
                .windows(3)
                .any(|w| w[0] == "clippy" && w[1] == "::" && SILENCED.contains(&w[2]));
            let all = t.text != "warn"
                && words
                    .iter()
                    .enumerate()
                    .any(|(n, w)| *w == "warnings" && (n == 0 || words[n - 1] != "::"));
            if named || all {
                out.push(format!(
                    "{path}:{}: {}({}) would silence clippy::too_many_lines; split the function into named steps",
                    t.line,
                    t.text,
                    words[1..].concat()
                ));
            }
        }
        i = end;
    }
    out
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for path in crate::shape::tracked(root)? {
        let src = std::fs::read_to_string(root.join(&path)).unwrap_or_default();
        out.extend(silenced_problems(&path, &src));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
