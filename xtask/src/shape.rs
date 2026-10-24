use std::path::Path;
use std::process::Command;

pub const MAX_CODE: usize = 600;
pub const MAX_INLINE_TESTS: usize = 300;
pub const MAX_TEST_FILE: usize = 1000;
pub const MAX_HEADER: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Product,
    Test,
}

pub fn header_files(root: &Path) -> Vec<String> {
    let mut out = vec!["src/lib.rs".to_string(), "src/main.rs".to_string()];
    for g in crate::layers::GROUPS {
        out.push(format!("src/{g}/mod.rs"));
    }
    out.push("xtask/src/main.rs".to_string());
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        let mut crates: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .collect();
        crates.sort();
        for c in crates {
            for f in ["lib.rs", "main.rs"] {
                if root.join("crates").join(&c).join("src").join(f).exists() {
                    out.push(format!("crates/{c}/src/{f}"));
                }
            }
        }
    }
    out
}

pub fn kind_of(path: &str, src: &str) -> Kind {
    let test_path = path.starts_with("tests/")
        || path.contains("/tests/")
        || path == "tests.rs"
        || path.ends_with("/tests.rs");
    let test_only = src
        .lines()
        .find(|l| !l.trim().is_empty())
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

pub fn comment_problems(path: &str, src: &str, header_allowed: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut header = 0;
    let mut in_header = header_allowed;
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if in_header && line.starts_with("//!") {
            header += 1;
            if header > MAX_HEADER {
                out.push(format!(
                    "{path}:{}: the //! header has more than {MAX_HEADER} lines",
                    i + 1
                ));
            }
            continue;
        }
        in_header = false;
        if t.starts_with("//") {
            out.push(format!(
                "{path}:{}: a comment; code has no comments, only a //! header at the top of a crate root or layer mod.rs",
                i + 1
            ));
        }
    }
    if header_allowed && header == 0 {
        out.push(format!(
            "{path}: needs a //! header of at most {MAX_HEADER} lines saying what it is for and what it must not depend on"
        ));
    }
    out
}

fn tracked(root: &Path) -> Result<Vec<String>, String> {
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
    let headers = header_files(root);
    let mut out = Vec::new();
    for path in tracked(root)? {
        let Ok(src) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        out.extend(size_problems(&path, &src));
        out.extend(comment_problems(&path, &src, headers.contains(&path)));
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
    fn a_crate_root_or_layer_mod_has_a_header_of_at_most_three_lines() {
        let ok = "//! one\n//! two\n//! three\n\nmod a;\n";
        assert!(comment_problems("src/lib.rs", ok, true).is_empty());
        let long = "//! one\n//! two\n//! three\n//! four\nmod a;\n";
        assert_eq!(comment_problems("src/lib.rs", long, true).len(), 1);
        let missing = "mod a;\n";
        assert_eq!(comment_problems("src/lib.rs", missing, true).len(), 1);
        let late = "mod a;\n//! late\n";
        assert_eq!(comment_problems("src/lib.rs", late, true).len(), 2);
        let other = "//! one\n// plain\nmod a;\n";
        assert_eq!(comment_problems("src/lib.rs", other, true).len(), 1);
    }

    #[test]
    fn the_header_files_are_the_crate_roots_and_the_layer_folders() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let h = header_files(root);
        for f in [
            "src/lib.rs",
            "src/main.rs",
            "src/base/mod.rs",
            "src/wire/mod.rs",
            "src/policy/mod.rs",
            "src/app/mod.rs",
            "xtask/src/main.rs",
            "crates/burrow-config/src/lib.rs",
            "crates/burrow-admin-ui/src/main.rs",
        ] {
            assert!(h.iter().any(|x| x == f), "{f} in {h:?}");
        }
        assert!(!h.iter().any(|x| x == "src/config/mod.rs"));
    }
}
