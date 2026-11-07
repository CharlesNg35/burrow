use std::path::Path;

use crate::shape::{Kind, inline_tests_at, kind_of};

const SCOPE: &[&str] = &[
    "crates/burrow-admin-ui/src",
    "crates/burrow-ui-kit/src",
    "crates/burrow-admin-logic/src",
];

const DRAWN_ELSEWHERE: &[char] = &['→', '←'];

fn in_font(c: char) -> bool {
    let n = c as u32;
    n < 0x100
        || (0x2000..=0x206F).contains(&n)
        || [0x20AC, 0x2122, 0x2191, 0x2193, 0x2212, 0x2215].contains(&n)
        || DRAWN_ELSEWHERE.contains(&c)
}

pub fn file_problems(path: &str, src: &str) -> Vec<String> {
    if kind_of(path, src) == Kind::Test || !SCOPE.iter().any(|s| path.starts_with(&format!("{s}/")))
    {
        return Vec::new();
    }
    let lines: Vec<&str> = src.lines().collect();
    let end = inline_tests_at(&lines).unwrap_or(lines.len());
    lines[..end]
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            l.chars().find(|c| !in_font(*c)).map(|c| {
                format!(
                    "{path}:{}: `{c}` (U+{:04X}) is not in the admin UI's fonts and shows as an empty box or another font; use an Icon or words (docs/admin-ui-dev.md, rule 9)",
                    i + 1,
                    c as u32
                )
            })
        })
        .collect()
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for path in crate::shape::tracked(root)? {
        let Ok(src) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        out.extend(file_problems(&path, &src));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symbol_the_fonts_lack_is_refused_in_ui_code_only() {
        let src = "fn a() -> &'static str { \"Release notes ↗\" }\n";
        let p = file_problems("crates/burrow-admin-ui/src/a.rs", src);
        assert_eq!(p.len(), 1);
        assert!(
            p[0].starts_with("crates/burrow-admin-ui/src/a.rs:1: `↗`"),
            "{p:?}"
        );
        assert!(file_problems("src/cli.rs", src).is_empty());
        let ok = "const T: &str = \"Open websites → · 2h 5m … café ↑\";\n";
        assert!(file_problems("crates/burrow-ui-kit/src/a.rs", ok).is_empty());
        let tests = "fn a() {}\n#[cfg(test)]\nmod tests {\n    const X: &str = \"⚠\";\n}\n";
        assert!(file_problems("crates/burrow-admin-logic/src/a.rs", tests).is_empty());
    }
}
