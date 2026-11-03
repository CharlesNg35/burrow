use std::path::Path;

use crate::shape::{Kind, inline_tests_at, kind_of, lex};

pub struct Once {
    pub needle: &'static str,
    pub home: &'static str,
    pub scope: &'static [&'static str],
    pub why: &'static str,
}

pub const RULES: &[Once] = &[Once {
    needle: "clients.iter().find(",
    home: "crates/burrow-admin-logic/src/presence.rs",
    scope: &[
        "crates/burrow-admin-ui/src",
        "crates/burrow-admin-logic/src",
    ],
    why: "look a client's status up with burrow_admin_logic::presence",
}];

pub fn file_problems(path: &str, src: &str, rules: &[Once]) -> Vec<String> {
    if kind_of(path, src) == Kind::Test {
        return Vec::new();
    }
    let lines: Vec<&str> = src.lines().collect();
    let product = lines[..inline_tests_at(&lines).unwrap_or(lines.len())].join("\n");
    let code = lex(&product).code;
    let mut out = Vec::new();
    for r in rules {
        if path == r.home || !r.scope.iter().any(|s| path.starts_with(&format!("{s}/"))) {
            continue;
        }
        for at in hits(&code, r.needle) {
            out.push(format!(
                "{path}:{at}: `{}` only in {}; {}",
                r.needle, r.home, r.why
            ));
        }
    }
    out
}

fn hits(code: &str, needle: &str) -> Vec<usize> {
    let (mut flat, mut line_of) = (String::new(), Vec::new());
    let mut line = 1;
    for c in code.chars() {
        if c == '\n' {
            line += 1;
        }
        if !c.is_whitespace() {
            flat.push(c);
            line_of.resize(flat.len(), line);
        }
    }
    let needle: String = needle.chars().filter(|c| !c.is_whitespace()).collect();
    flat.match_indices(&needle)
        .map(|(i, _)| line_of[i])
        .collect()
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for path in crate::shape::tracked(root)? {
        let Ok(src) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        out.extend(file_problems(&path, &src, RULES));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &[Once] = &[Once {
        needle: "clients.iter().find(",
        home: "crates/l/src/presence.rs",
        scope: &["crates/u/src"],
        why: "use presence",
    }];

    #[test]
    fn a_pattern_is_allowed_only_in_its_home_and_outside_its_scope() {
        let src = "fn a(s: &S) { s.clients.iter().find(|c| c.ok); }\n";
        assert_eq!(file_problems("crates/u/src/a.rs", src, RULE).len(), 1);
        assert!(file_problems("crates/l/src/presence.rs", src, RULE).is_empty());
        assert!(file_problems("src/a.rs", src, RULE).is_empty());
        let split = "fn a() {}\nfn b(s: &S) { s.clients\n    .iter()\n    .find(|c| c.ok); }\n";
        let p = file_problems("crates/u/src/a.rs", split, RULE);
        assert_eq!(p.len(), 1);
        assert!(p[0].starts_with("crates/u/src/a.rs:2:"), "{p:?}");
    }

    #[test]
    fn tests_and_strings_do_not_count() {
        let tests = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn t(s: &S) { s.clients.iter().find(|c| c.ok); }\n}\n";
        assert!(file_problems("crates/u/src/a.rs", tests, RULE).is_empty());
        let text = "const T: &str = \"clients.iter().find(\";\n";
        assert!(file_problems("crates/u/src/a.rs", text, RULE).is_empty());
    }
}
