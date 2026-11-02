use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

pub const MAX_CODE: usize = 600;
pub const MAX_INLINE_TESTS: usize = 300;
pub const MAX_TEST_FILE: usize = 1000;
pub const MAX_HEADER: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Product,
    Test,
}

pub fn module_roots(files: &[String]) -> BTreeSet<String> {
    let folders: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| f.rsplit_once('/').map(|(d, _)| d))
        .collect();
    let has_folder = |stem: &str| {
        folders
            .iter()
            .any(|d| *d == stem || d.strip_prefix(stem).is_some_and(|r| r.starts_with('/')))
    };
    files
        .iter()
        .filter(|f| {
            let name = f.rsplit_once('/').map_or(f.as_str(), |(_, n)| n);
            matches!(name, "lib.rs" | "main.rs" | "mod.rs")
                || f.strip_suffix(".rs").is_some_and(has_folder)
        })
        .cloned()
        .collect()
}

pub fn kind_of(path: &str, src: &str) -> Kind {
    let test_path = path.starts_with("tests/")
        || path.contains("/tests/")
        || path == "tests.rs"
        || path.ends_with("/tests.rs");
    let test_only = src
        .lines()
        .find(|l| !l.trim().is_empty() && !l.starts_with("//!"))
        .is_some_and(|l| l.trim() == "#![cfg(test)]");
    if test_path || test_only {
        Kind::Test
    } else {
        Kind::Product
    }
}

pub fn inline_tests_at(lines: &[&str]) -> Option<usize> {
    lines.iter().enumerate().find_map(|(i, l)| {
        let gated = l.starts_with("#[cfg(test)]")
            && lines.get(i + 1).is_some_and(|n| opens_module(n))
            && trailing(lines, i + 1);
        let plain = l.starts_with("mod tests {") && trailing(lines, i);
        (gated || plain).then_some(i)
    })
}

fn opens_module(line: &str) -> bool {
    let rest = ["pub(crate) ", "pub(super) ", "pub "]
        .iter()
        .find_map(|v| line.strip_prefix(v))
        .unwrap_or(line);
    rest.strip_prefix("mod ")
        .and_then(|r| r.strip_suffix(" {"))
        .is_some_and(|name| {
            !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
}

fn trailing(lines: &[&str], open: usize) -> bool {
    lines[open + 1..]
        .iter()
        .position(|l| *l == "}")
        .is_some_and(|end| lines[open + 2 + end..].iter().all(|l| l.trim().is_empty()))
}

pub fn size_problems(path: &str, src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let total = lines.len();
    let mut out = Vec::new();
    match kind_of(path, src) {
        Kind::Test => {
            if total > MAX_TEST_FILE {
                out.push(format!(
                    "{path}: a test file has {total} lines; at most {MAX_TEST_FILE}, so make it a folder of modules"
                ));
            }
        }
        Kind::Product => {
            let at = inline_tests_at(&lines);
            let code = at.unwrap_or(total);
            if code > MAX_CODE {
                out.push(format!(
                    "{path}: {code} lines of code; at most {MAX_CODE}, so split it by responsibility"
                ));
            }
            if let Some(at) = at
                && total - at > MAX_INLINE_TESTS
            {
                out.push(format!(
                    "{path}: the inline test module has {} lines; at most {MAX_INLINE_TESTS}, so move it to a sibling tests.rs",
                    total - at
                ));
            }
        }
    }
    out
}

pub struct Lexed {
    pub code: String,
    pub comments: Vec<(usize, usize, String)>,
}

fn ident_char(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric() || *c == '_')
}

fn raw_start(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if chars.get(i) == Some(&'b') {
        if chars.get(j) != Some(&'r') {
            return None;
        }
        j += 1;
    } else if chars.get(i) != Some(&'r') {
        return None;
    }
    if i > 0 && ident_char(chars.get(i - 1)) {
        return None;
    }
    let mut hashes = 0;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some(hashes)
}

pub fn lex(src: &str) -> Lexed {
    let chars: Vec<char> = src.chars().collect();
    let mut code = String::with_capacity(src.len());
    let mut comments = Vec::new();
    let (mut line, mut col) = (1, 0);
    let mut i = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let end = if c == '/' && chars.get(i + 1) == Some(&'/') {
            let mut j = i;
            while j < chars.len() && chars[j] != '\n' {
                j += 1;
            }
            comments.push((line, col, chars[i..j].iter().collect()));
            j
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut depth = 0;
            let mut j = i;
            while j < chars.len() {
                if chars[j] == '/' && chars.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            comments.push((line, col, chars[i..j.min(chars.len())].iter().collect()));
            j
        } else if let Some(hashes) = raw_start(&chars, i) {
            let open = chars[i..]
                .iter()
                .position(|c| *c == '"')
                .map_or(i, |p| i + p);
            let mut j = open + 1;
            loop {
                if j >= chars.len() {
                    break j;
                }
                if chars[j] == '"' && (0..hashes).all(|h| chars.get(j + 1 + h) == Some(&'#')) {
                    break j + 1 + hashes;
                }
                j += 1;
            }
        } else if c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            j + 1
        } else if c == '\'' {
            let rest: String = chars[i..chars.len().min(i + 12)].iter().collect();
            let len = char_literal_len(&rest);
            if len == 0 { i + 1 } else { i + len }
        } else {
            i + 1
        };
        let end = end.min(chars.len());
        let literal = end > start + 1;
        for &ch in &chars[start..end] {
            code.push(if literal { blank(ch) } else { ch });
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        i = end;
    }
    Lexed { code, comments }
}

fn char_literal_len(rest: &str) -> usize {
    let b: Vec<char> = rest.chars().collect();
    match (b.get(1), b.get(2), b.get(3)) {
        (Some('\\'), Some('u'), Some('{')) => b
            .iter()
            .position(|c| *c == '}')
            .filter(|p| b.get(p + 1) == Some(&'\''))
            .map_or(0, |p| p + 2),
        (Some('\\'), Some(_), Some('\'')) => 4,
        (Some(c), Some('\''), _) if *c != '\\' && *c != '\n' => 3,
        _ => 0,
    }
}

fn squashed_lines(code: &str) -> Vec<String> {
    code.lines()
        .map(|l| l.chars().filter(|c| !c.is_whitespace()).collect())
        .collect()
}

fn lines_with(code: &str, needle: &str) -> Vec<usize> {
    squashed_lines(code)
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, _)| i + 1)
        .collect()
}

pub fn comment_problems(path: &str, src: &str, root: bool) -> Vec<String> {
    let lexed = lex(src);
    let header_lines = if root {
        src.lines().take_while(|l| l.starts_with("//!")).count()
    } else {
        0
    };
    let mut out = Vec::new();
    for (line, col, text) in &lexed.comments {
        if *line <= header_lines && *col == 0 && text.starts_with("//!") {
            if *line > MAX_HEADER {
                out.push(format!(
                    "{path}:{line}: the //! header has more than {MAX_HEADER} lines"
                ));
            }
            continue;
        }
        let rule = if root {
            "code has no comments, only the //! header at the top"
        } else {
            "code has no comments, and only a module root (lib.rs, main.rs, mod.rs, or foo.rs with a foo/ folder) has a //! header"
        };
        out.push(format!("{path}:{line}: a comment; {rule}"));
    }
    for at in lines_with(&lexed.code, "#[doc=")
        .into_iter()
        .chain(lines_with(&lexed.code, "#![doc="))
    {
        out.push(format!(
            "{path}:{at}: a doc attribute; code has no comments or docs"
        ));
    }
    if root {
        out.extend(header_shape(path, src, header_lines));
    }
    out
}

fn header_shape(path: &str, src: &str, header_lines: usize) -> Option<String> {
    let first = src.lines().next().unwrap_or_default();
    let summary = first.trim_start_matches("//!").trim();
    if header_lines == 0 {
        Some(format!(
            "{path}: a module root needs a //! header of at most {MAX_HEADER} lines, its first line a one-sentence summary"
        ))
    } else if !summary.ends_with('.') {
        Some(format!(
            "{path}:1: the //! header's first line is a whole sentence of its own, ending with a full stop"
        ))
    } else {
        None
    }
}

pub(crate) fn tracked(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "-z", "--", "*.rs"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn problems(root: &Path) -> Result<Vec<String>, String> {
    let files = tracked(root)?;
    let roots = module_roots(&files);
    let mut out = Vec::new();
    for path in files {
        let Ok(src) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        out.extend(size_problems(&path, &src));
        out.extend(comment_problems(&path, &src, roots.contains(&path)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize, text: &str) -> String {
        std::iter::repeat_n(text, n).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_product_file_may_have_600_lines_of_code() {
        assert!(size_problems("src/a.rs", &lines(600, "let x = 1;")).is_empty());
        assert_eq!(
            size_problems("src/a.rs", &lines(601, "let x = 1;")).len(),
            1
        );
    }

    #[test]
    fn the_inline_test_module_does_not_count_as_code() {
        let src = format!(
            "{}\n#[cfg(test)]\nmod tests {{\n{}\n}}",
            lines(590, "let x = 1;"),
            lines(200, "fn t() {}")
        );
        assert!(size_problems("src/a.rs", &src).is_empty());
    }

    #[test]
    fn a_test_module_of_any_visibility_or_name_is_the_inline_test_module() {
        for head in [
            "pub(crate) mod tests {",
            "pub mod tests {",
            "pub(super) mod checks {",
        ] {
            let src = format!(
                "{}\n#[cfg(test)]\n{head}\n{}\n}}\n",
                lines(455, "let x = 1;"),
                lines(270, "fn t() {}")
            );
            assert!(size_problems("src/a.rs", &src).is_empty(), "{head}");
            let lines_of: Vec<&str> = src.lines().collect();
            assert_eq!(inline_tests_at(&lines_of), Some(455), "{head}");
        }
    }

    #[test]
    fn a_gated_module_that_is_not_last_counts_as_code() {
        let src = format!(
            "#[cfg(test)]\nmod helpers {{\n}}\n{}",
            lines(600, "let x = 1;")
        );
        assert_eq!(size_problems("src/a.rs", &src).len(), 1);
    }

    #[test]
    fn an_inline_test_module_may_have_300_lines() {
        let ok = format!(
            "fn a() {{}}\n#[cfg(test)]\nmod tests {{\n{}\n}}",
            lines(297, "fn t() {}")
        );
        assert!(size_problems("src/a.rs", &ok).is_empty());
        let long = format!(
            "fn a() {{}}\n#[cfg(test)]\nmod tests {{\n{}\n}}",
            lines(298, "fn t() {}")
        );
        let p = size_problems("src/a.rs", &long);
        assert_eq!(p.len(), 1);
        assert!(p[0].contains("tests.rs"), "{p:?}");
    }

    #[test]
    fn a_test_file_may_have_1000_lines() {
        for path in [
            "tests/a.rs",
            "tests/a/b.rs",
            "src/x/tests.rs",
            "crates/c/tests/a.rs",
        ] {
            assert!(size_problems(path, &lines(1000, "fn t() {}")).is_empty());
            assert_eq!(
                size_problems(path, &lines(1001, "fn t() {}")).len(),
                1,
                "{path}"
            );
        }
    }

    #[test]
    fn a_test_only_file_is_a_test_file() {
        let src = format!("#![cfg(test)]\n\n{}", lines(900, "fn t() {}"));
        assert_eq!(kind_of("src/a/agree.rs", &src), Kind::Test);
        assert!(size_problems("src/a/agree.rs", &src).is_empty());
    }

    #[test]
    fn code_has_no_comments() {
        assert!(comment_problems("src/a.rs", "fn a() {}\n", false).is_empty());
        for c in ["// note", "    // note", "/// doc", "//! header"] {
            let src = format!("fn a() {{}}\n{c}\n");
            assert_eq!(comment_problems("src/a.rs", &src, false).len(), 1, "{c}");
        }
        assert_eq!(
            comment_problems("src/a.rs", "//! header\nfn a() {}\n", false).len(),
            1
        );
    }

    #[test]
    fn trailing_block_and_doc_comments_are_found() {
        for c in [
            "let x = 1; // note",
            "let x = 1; /* note */",
            "/* note */",
            "/** doc */",
            "/*! inner doc */",
            "/* outer /* nested */ still */",
            "#[doc = \"text\"]",
            "#![doc = \"text\"]",
            "# [ doc = \"text\" ]",
        ] {
            let src = format!("fn a() {{}}\n{c}\nfn b() {{}}\n");
            assert_eq!(comment_problems("src/a.rs", &src, false).len(), 1, "{c}");
        }
        assert!(comment_problems("src/a.rs", "#[doc(hidden)]\npub fn a() {}\n", false).is_empty());
    }

    #[test]
    fn literals_and_lifetimes_are_not_comments() {
        for c in [
            "let u = \"https://example.com/a\";",
            "let s = \"/* not a comment */ // either\";",
            "let e = \"a \\\" // still a string\";",
            "let r = r#\"// raw \"quoted\" /* text */\"#;",
            "let b = b\"//\";",
            "let br = br\"//\";",
            "let slash = '/';",
            "let quote = '\"';",
            "let tick = '\\'';",
            "let esc = '\\u{2f}';",
            "fn f<'a>(x: &'a str) -> &'a str { x }",
            "let d = 6 / 2 / 1;",
        ] {
            let src = format!("{c}\n");
            assert!(comment_problems("src/a.rs", &src, false).is_empty(), "{c}");
        }
        let after = "let u = \"https://x\"; // real\n";
        assert_eq!(comment_problems("src/a.rs", after, false).len(), 1);
    }

    #[test]
    fn a_module_root_has_a_header_of_at_most_five_lines_led_by_a_sentence() {
        let ok = "//! One.\n//! two\n//! three\n//! four\n//! five\n\nmod a;\n";
        assert!(comment_problems("src/lib.rs", ok, true).is_empty());
        let long = "//! One.\n//! two\n//! three\n//! four\n//! five\n//! six\nmod a;\n";
        assert_eq!(comment_problems("src/lib.rs", long, true).len(), 1);
        let missing = "mod a;\n";
        assert_eq!(comment_problems("src/lib.rs", missing, true).len(), 1);
        let late = "mod a;\n//! Late.\n";
        assert_eq!(comment_problems("src/lib.rs", late, true).len(), 2);
        let other = "//! One.\n// plain\nmod a;\n";
        assert_eq!(comment_problems("src/lib.rs", other, true).len(), 1);
        let cut = "//! One sentence cut\n//! in two.\nmod a;\n";
        assert_eq!(comment_problems("src/lib.rs", cut, true).len(), 1);
    }

    #[test]
    fn a_header_above_cfg_test_keeps_the_file_a_test_file() {
        let src = "//! Shared test helpers.\n\n#![cfg(test)]\nfn t() {}\n";
        assert_eq!(kind_of("src/a/kit.rs", src), Kind::Test);
    }

    #[test]
    fn module_roots_are_lib_main_mod_and_files_with_a_folder() {
        let files: Vec<String> = [
            "src/lib.rs",
            "src/main.rs",
            "src/base/mod.rs",
            "src/base/fs.rs",
            "src/config/web.rs",
            "src/config/web/fill.rs",
            "src/config/webby.rs",
            "tests/admin.rs",
            "tests/admin/flows.rs",
            "tests/common/mod.rs",
            "tests/plain.rs",
            "crates/c/src/lib.rs",
            "crates/c/tests/h2/main.rs",
            "xtask/src/main.rs",
        ]
        .map(String::from)
        .to_vec();
        let roots = module_roots(&files);
        for f in [
            "src/lib.rs",
            "src/main.rs",
            "src/base/mod.rs",
            "src/config/web.rs",
            "tests/admin.rs",
            "tests/common/mod.rs",
            "crates/c/src/lib.rs",
            "crates/c/tests/h2/main.rs",
            "xtask/src/main.rs",
        ] {
            assert!(roots.contains(f), "{f} in {roots:?}");
        }
        for f in [
            "src/base/fs.rs",
            "src/config/webby.rs",
            "src/config/web/fill.rs",
            "tests/plain.rs",
        ] {
            assert!(!roots.contains(f), "{f} is a leaf");
        }
    }
}
