use std::path::Path;
use std::process::Command;

const CRATE_DEPS: &[(&str, &[&str], &[&str])] = &[
    ("burrow-syntax", &[], &[]),
    (
        "burrow-admin-proto",
        &["serde", "serde_json", "schemars"],
        &[],
    ),
    ("burrow-admin-derive", &["proc-macro2", "quote", "syn"], &[]),
    (
        "burrow-config",
        &[
            "serde",
            "burrow-admin-proto",
            "burrow-admin-derive",
            "burrow-syntax",
            "schemars",
        ],
        &[
            "burrow",
            "tokio",
            "burrow-ui-kit",
            "burrow-admin-logic",
            "burrow-admin-ui",
            "burrow-web",
        ],
    ),
    (
        "burrow-admin-logic",
        &[
            "burrow-admin-proto",
            "burrow-config",
            "burrow-syntax",
            "semver",
        ],
        &["burrow-web"],
    ),
    (
        "burrow-ui-kit",
        &["leptos", "wasm-bindgen", "web-sys", "js-sys"],
        &[
            "burrow-admin-proto",
            "burrow-syntax",
            "burrow-admin-logic",
            "burrow-web",
        ],
    ),
    (
        "burrow-admin-ui",
        &[
            "leptos",
            "wasm-bindgen",
            "wasm-bindgen-futures",
            "web-sys",
            "js-sys",
            "burrow-admin-proto",
            "burrow-admin-logic",
            "burrow-config",
            "burrow-ui-kit",
            "burrow-syntax",
            "console_error_panic_hook",
        ],
        &["serde_json", "burrow-web"],
    ),
    (
        "burrow-web",
        &[
            "base64",
            "burrow-syntax",
            "bytes",
            "http",
            "http-body-util",
            "httpdate",
            "hyper",
            "hyper-util",
            "instant-acme",
            "ppp",
            "rcgen",
            "ring",
            "rustls-webpki",
            "serde",
            "serde_json",
            "socket2",
            "tokio",
            "tokio-rustls",
            "tracing",
            "webpki-roots",
            "x509-cert",
            "zeroize",
        ],
        &[
            "burrow",
            "burrow-config",
            "burrow-admin-proto",
            "burrow-admin-derive",
            "burrow-admin-logic",
            "burrow-admin-ui",
            "burrow-ui-kit",
        ],
    ),
];

const ROOT_NEVER: &[&str] = &["burrow-ui-kit", "burrow-admin-logic", "burrow-admin-ui"];

pub fn run(root: &Path) -> Result<(), String> {
    let mut problems = check_deps(root)?;
    problems.extend(crate::fields::problems(root)?);
    problems.extend(crate::layers::problems(root)?);
    problems.extend(crate::shape::problems(root)?);
    problems.extend(crate::lints::problems(root)?);
    if problems.is_empty() {
        println!("arch-check: ok");
        Ok(())
    } else {
        for p in &problems {
            eprintln!("{p}");
        }
        Err(format!("arch-check: {} violations", problems.len()))
    }
}

fn check_deps(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut problems = Vec::new();
    let empty = Vec::new();
    for pkg in meta["packages"].as_array().unwrap_or(&empty) {
        let name = pkg["name"].as_str().unwrap_or_default();
        let deps: Vec<&str> = pkg["dependencies"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|d| d["kind"].is_null())
            .filter_map(|d| d["name"].as_str())
            .collect();
        problems.extend(dep_problems(name, &deps));
    }
    Ok(problems)
}

fn dep_problems(name: &str, deps: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    if name == "burrow" {
        for d in deps.iter().filter(|d| ROOT_NEVER.contains(d)) {
            problems.push(format!(
                "crate burrow must not depend on {d} (UI crates are wasm only)"
            ));
        }
    }
    if let Some((_, allowed, never)) = CRATE_DEPS.iter().find(|(n, _, _)| *n == name) {
        for d in deps {
            if never.contains(d) {
                problems.push(format!("crate {name} must not depend on {d}"));
            } else if !allowed.contains(d) {
                problems.push(format!(
                    "crate {name} may depend only on [{}], not on {d}",
                    allowed.join(", ")
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_rules() {
        assert!(dep_problems("burrow-admin-proto", &["serde", "serde_json"]).is_empty());
        assert_eq!(
            dep_problems("burrow-admin-proto", &["serde", "tokio"]).len(),
            1
        );
        assert_eq!(
            dep_problems("burrow-ui-kit", &["leptos", "burrow-admin-proto"]).len(),
            1
        );
        assert_eq!(dep_problems("burrow", &["burrow-admin-ui"]).len(), 1);
        assert!(
            dep_problems(
                "burrow-config",
                &[
                    "serde",
                    "burrow-admin-proto",
                    "burrow-admin-derive",
                    "schemars"
                ]
            )
            .is_empty()
        );
        assert_eq!(dep_problems("burrow-config", &["serde", "tokio"]).len(), 1);
        assert_eq!(dep_problems("burrow-config", &["burrow"]).len(), 1);
        assert!(dep_problems("burrow-web", &["tokio", "hyper", "burrow-syntax"]).is_empty());
        for never in [
            "burrow",
            "burrow-config",
            "burrow-admin-proto",
            "burrow-ui-kit",
        ] {
            assert_eq!(
                dep_problems("burrow-web", &[never]),
                [format!("crate burrow-web must not depend on {never}")]
            );
        }
        for ui in [
            "burrow-config",
            "burrow-admin-logic",
            "burrow-ui-kit",
            "burrow-admin-ui",
        ] {
            assert_eq!(
                dep_problems(ui, &["burrow-web"]),
                [format!("crate {ui} must not depend on burrow-web")]
            );
        }
    }
}
