use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use sha2::Digest;

use crate::{cargo, target_dir};

const USAGE: &str = "usage: cargo xtask surface [--out FILE] [--check FILE]";
const SHELLS: &[&str] = &["bash", "elvish", "fish", "powershell", "zsh"];
const ROLES: &[&str] = &["server", "client", "visitor"];
const FIXTURE: &str = "crates/burrow-admin-proto/tests/fixtures/v0.1/schema.json";
const SIZE_TOLERANCE: f64 = 0.01;

pub type Sections = BTreeMap<String, Vec<String>>;

fn output(cmd: &mut Command) -> Result<String, String> {
    let shown = std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{shown} failed ({})", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{shown}: {e}"))
}

fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect()
}

pub fn render(sections: &Sections) -> String {
    let mut out = String::new();
    for (name, body) in sections {
        out.push_str("=== ");
        out.push_str(name);
        out.push('\n');
        for l in body {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

pub fn parse(text: &str) -> Sections {
    let mut sections = Sections::new();
    let mut current: Option<String> = None;
    for l in text.lines() {
        if let Some(name) = l.strip_prefix("=== ") {
            current = Some(name.to_string());
            sections.entry(name.to_string()).or_default();
        } else if let Some(name) = &current {
            sections
                .entry(name.clone())
                .or_default()
                .push(l.to_string());
        }
    }
    sections
}

pub fn commands_in(help: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut inside = false;
    for l in help.lines() {
        if l == "Commands:" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if l.trim().is_empty() || !l.starts_with("  ") {
            break;
        }
        if l.starts_with("   ") {
            continue;
        }
        if let Some(name) = l.split_whitespace().next() {
            found.push(name.trim_end_matches(',').to_string());
        }
    }
    found
}

fn burrow(bin: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", "/nonexistent-home")
        .env("LANG", "C")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    output(&mut cmd)
}

fn walk_help(bin: &Path, path: &[String], sections: &mut Sections) -> Result<(), String> {
    let args: Vec<&str> = path.iter().map(String::as_str).collect();
    let name = std::iter::once("burrow")
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let long = burrow(bin, &[&args[..], &["--help"]].concat())?;
    let short = burrow(bin, &[&args[..], &["-h"]].concat())?;
    sections.insert(format!("help {name}"), lines(&long));
    sections.insert(format!("help-short {name}"), lines(&short));
    for sub in commands_in(&long).into_iter().filter(|c| c != "help") {
        let mut next = path.to_vec();
        next.push(sub);
        walk_help(bin, &next, sections)?;
    }
    Ok(())
}

fn cli(bin: &Path, sections: &mut Sections) -> Result<(), String> {
    walk_help(bin, &[], sections)?;
    sections.insert(
        "version".into(),
        lines(&(burrow(bin, &["--version"])? + &burrow(bin, &["version"])?)),
    );
    for role in ROLES {
        sections.insert(
            format!("schema {role}"),
            lines(&burrow(bin, &["schema", role])?),
        );
    }
    for shell in SHELLS {
        sections.insert(
            format!("completions {shell}"),
            lines(&burrow(bin, &["completions", shell])?),
        );
    }
    Ok(())
}

pub fn fixture_lines(text: &str) -> Vec<String> {
    let text = text.replace("\r\n", "\n");
    let hash = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v["hash"].as_str().map(str::to_string))
        .unwrap_or_else(|| "none".into());
    vec![
        format!(
            "sha256 {}",
            hex::encode(sha2::Sha256::digest(text.as_bytes()))
        ),
        format!("schema hash {hash}"),
    ]
}

pub fn fuzz_targets(manifest: &str, files: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_bin = false;
    for l in manifest.lines().map(str::trim) {
        if l.starts_with('[') {
            in_bin = l == "[[bin]]";
            continue;
        }
        if in_bin
            && let Some(v) = l.strip_prefix("name")
            && let Some(v) = v.trim_start().strip_prefix('=')
        {
            out.push(format!("bin {}", v.trim().trim_matches('"')));
        }
    }
    let mut files: Vec<String> = files.iter().map(|f| format!("file {f}")).collect();
    files.sort();
    out.sort();
    out.extend(files);
    out
}

fn fuzz(root: &Path) -> Result<Vec<String>, String> {
    let manifest = std::fs::read_to_string(root.join("fuzz/Cargo.toml"))
        .map_err(|e| format!("fuzz/Cargo.toml: {e}"))?;
    let dir = root.join("fuzz/fuzz_targets");
    let files: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    Ok(fuzz_targets(&manifest, &files))
}

pub struct TestBinary {
    pub key: String,
    pub exe: PathBuf,
}

pub fn test_binaries(root: &Path, json: &str) -> Vec<TestBinary> {
    let mut out: Vec<TestBinary> = Vec::new();
    for m in json
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    {
        if m["reason"] != "compiler-artifact" || m["profile"]["test"] != true {
            continue;
        }
        let Some(exe) = m["executable"].as_str() else {
            continue;
        };
        let kind = m["target"]["kind"][0].as_str().unwrap_or("?");
        if kind == "bench" {
            continue;
        }
        let manifest = Path::new(m["manifest_path"].as_str().unwrap_or_default());
        let dir = manifest
            .parent()
            .and_then(|d| d.strip_prefix(root).ok())
            .map(|d| d.to_string_lossy().replace('\\', "/"))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| ".".into());
        let key = format!(
            "{dir} {kind} {}",
            m["target"]["name"].as_str().unwrap_or("?")
        );
        if !out.iter().any(|b| b.key == key) {
            out.push(TestBinary {
                key,
                exe: PathBuf::from(exe),
            });
        }
    }
    out
}

pub fn listed(key: &str, list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|l| l.strip_suffix(": test"))
        .map(|name| format!("[{key}] {name}"))
        .collect()
}

fn list_tests(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<Vec<String>, String> {
    let json = output(
        cargo()
            .current_dir(root)
            .args(["test", "--locked", "--all-targets", "--no-run"])
            .args(args)
            .arg("--message-format=json"),
    )?;
    let mut names = Vec::new();
    for b in test_binaries(root, &json) {
        let mut cmd = Command::new(&b.exe);
        cmd.current_dir(root).arg("--list");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        names.extend(listed(&b.key, &output(&mut cmd)?));
    }
    names.sort();
    Ok(names)
}

pub fn public_items(json: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = json["paths"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, p)| p["crate_id"] == 0)
        .filter_map(|(_, p)| {
            let path: Vec<&str> = p["path"]
                .as_array()?
                .iter()
                .filter_map(|s| s.as_str())
                .collect();
            Some(format!(
                "{} {}",
                p["kind"].as_str().unwrap_or("?"),
                path.join("::")
            ))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn rustdoc(root: &Path) -> Vec<String> {
    let nightly = Command::new("cargo")
        .args(["+nightly", "--version"])
        .output();
    if !nightly.is_ok_and(|o| o.status.success()) {
        return vec!["skipped: no nightly toolchain".into()];
    }
    let built = Command::new("cargo")
        .current_dir(root)
        .env_remove("RUSTC")
        .env_remove("RUSTDOC")
        .args([
            "+nightly",
            "rustdoc",
            "--locked",
            "-p",
            "burrow",
            "--lib",
            "--",
            "-Z",
            "unstable-options",
            "--output-format",
            "json",
        ])
        .stdout(Stdio::null())
        .status();
    if !built.is_ok_and(|s| s.success()) {
        return vec!["skipped: cargo +nightly rustdoc failed".into()];
    }
    let file = target_dir(root).join("doc/burrow.json");
    match std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    {
        Some(v) => public_items(&v),
        None => vec![format!("skipped: cannot read {}", file.display())],
    }
}

fn sizes(root: &Path) -> Result<(Vec<String>, PathBuf), String> {
    crate::ui::main(root, &["--release".to_string()])?;
    let manifest = crate::ui::dist_dir(root).join("manifest.json");
    let text =
        std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let m: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let ui = crate::size::measure(&m);
    let wasm_raw: u64 = m["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["path"].as_str().unwrap_or_default().ends_with(".wasm"))
        .filter_map(|f| f["size"].as_u64())
        .sum();
    output(cargo().current_dir(root).args([
        "build",
        "--release",
        "--locked",
        "-p",
        "burrow",
        "--bin",
        "burrow",
    ]))?;
    let bin = target_dir(root)
        .join("release")
        .join(format!("burrow{}", std::env::consts::EXE_SUFFIX));
    let binary = std::fs::metadata(&bin)
        .map_err(|e| format!("{}: {e}", bin.display()))?
        .len();
    Ok((
        vec![
            format!("release-binary {binary}"),
            format!("wasm-gzip {}", ui.wasm),
            format!("wasm-raw {wasm_raw}"),
        ],
        bin,
    ))
}

fn step<T>(name: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let t = Instant::now();
    println!("surface: {name}");
    let r = f();
    println!("surface: {name} took {} s", t.elapsed().as_secs());
    r
}

pub fn collect(root: &Path) -> Result<Sections, String> {
    let mut s = Sections::new();
    let (size, bin) = step("sizes", || sizes(root))?;
    s.insert("sizes".into(), size);
    step("help, schema and completions", || cli(&bin, &mut s))?;
    let fixture =
        std::fs::read_to_string(root.join(FIXTURE)).map_err(|e| format!("{FIXTURE}: {e}"))?;
    s.insert("admin schema fixture".into(), fixture_lines(&fixture));
    s.insert("fuzz targets".into(), fuzz(root)?);
    let default = step("tests default", || list_tests(root, &["--workspace"], &[]))?;
    s.insert("tests default".into(), default);
    let ws = step("tests ws", || {
        list_tests(root, &["--workspace"], &[("BURROW_TEST_TRANSPORT", "ws")])
    })?;
    s.insert("tests ws".into(), ws);
    let hooks = step("tests admin-test-hooks", || {
        list_tests(
            root,
            &["-p", "burrow", "--features", "admin-test-hooks"],
            &[],
        )
    })?;
    s.insert("tests admin-test-hooks".into(), hooks);
    let items = step("public items", || Ok(rustdoc(root)))?;
    s.insert("public items".into(), items);
    Ok(s)
}

fn leaf(name: &str) -> (&str, &str) {
    let (key, path) = name.split_once("] ").unwrap_or(("", name));
    (key, path.rsplit("::").next().unwrap_or(path))
}

pub fn lost_tests(old: &[String], new: &[String]) -> Vec<String> {
    let mut gained: Vec<&String> = new.iter().filter(|n| !old.contains(n)).collect();
    let mut lost = Vec::new();
    for o in old.iter().filter(|o| !new.contains(o)) {
        match gained.iter().position(|g| leaf(g) == leaf(o)) {
            Some(i) => {
                gained.swap_remove(i);
            }
            None => lost.push(o.clone()),
        }
    }
    lost
}

fn number(lines: &[String], name: &str) -> Option<u64> {
    lines
        .iter()
        .find_map(|l| l.strip_prefix(name)?.strip_prefix(' ')?.parse().ok())
}

pub fn size_problems(old: &[String], new: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for l in old {
        let Some((name, _)) = l.split_once(' ') else {
            continue;
        };
        match (number(old, name), number(new, name)) {
            (Some(a), Some(b)) => {
                let change = (b as f64 - a as f64) / (a.max(1) as f64);
                if change.abs() > SIZE_TOLERANCE {
                    out.push(format!(
                        "{name} moved {:+.2}% ({a} B to {b} B), more than 1%",
                        change * 100.0
                    ));
                }
            }
            _ => out.push(format!("{name} is missing")),
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct Report {
    pub problems: Vec<String>,
    pub notes: Vec<String>,
}

fn diff_lines(old: &[String], new: &[String], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = old
        .iter()
        .filter(|l| !new.contains(l))
        .map(|l| format!("  - {l}"))
        .chain(
            new.iter()
                .filter(|l| !old.contains(l))
                .map(|l| format!("  + {l}")),
        )
        .collect();
    if out.len() > limit {
        let more = out.len() - limit;
        out.truncate(limit);
        out.push(format!("  ... {more} more"));
    }
    out
}

pub fn compare(old: &Sections, new: &Sections) -> Report {
    let mut r = Report::default();
    let empty = Vec::new();
    let names: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    for name in names {
        let (a, b) = (
            old.get(name).unwrap_or(&empty),
            new.get(name).unwrap_or(&empty),
        );
        if name.starts_with("tests ") {
            let lost = lost_tests(a, b);
            let gained = b.len().saturating_sub(a.len() - lost.len());
            if !lost.is_empty() {
                r.problems.push(format!(
                    "{name}: {} tests lost or renamed beyond a module move:",
                    lost.len()
                ));
                r.problems
                    .extend(lost.iter().take(50).map(|l| format!("  - {l}")));
            }
            if a != b {
                r.notes.push(format!(
                    "{name}: {} before, {} now, {gained} gained",
                    a.len(),
                    b.len()
                ));
            }
        } else if name == "sizes" {
            r.problems.extend(size_problems(a, b));
            if a != b {
                r.notes
                    .push(format!("sizes: {} -> {}", a.join(", "), b.join(", ")));
            }
        } else if name == "public items" {
            if a != b {
                r.notes.push("public items changed (not a failure):".into());
                r.notes.extend(diff_lines(a, b, 200));
            }
        } else if !old.contains_key(name) {
            r.problems.push(format!("{name}: new section"));
        } else if !new.contains_key(name) {
            r.problems.push(format!("{name}: section is gone"));
        } else if a != b {
            r.problems.push(format!("{name} differs:"));
            r.problems.extend(diff_lines(a, b, 40));
        }
    }
    r
}

pub fn main(root: &Path, args: &[String]) -> Result<(), String> {
    let mut out = target_dir(root).join("surface.txt");
    let mut check = None;
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(rest.next().ok_or(USAGE)?),
            "--check" => check = Some(PathBuf::from(rest.next().ok_or(USAGE)?)),
            other => return Err(format!("unknown argument {other}; {USAGE}")),
        }
    }
    let baseline = match &check {
        Some(f) => Some(std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?),
        None => None,
    };
    let started = Instant::now();
    let now = collect(root)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, render(&now)).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "surface: wrote {} in {} s",
        out.display(),
        started.elapsed().as_secs()
    );
    let Some(text) = baseline else {
        return Ok(());
    };
    let report = compare(&parse(&text), &now);
    for n in &report.notes {
        println!("{n}");
    }
    if report.problems.is_empty() {
        println!("surface: same as {}", check.unwrap_or_default().display());
        return Ok(());
    }
    for p in &report.problems {
        println!("{p}");
    }
    Err(format!(
        "surface: {} differences from the baseline",
        report
            .problems
            .iter()
            .filter(|p| !p.starts_with("  "))
            .count()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_command_list_comes_from_the_help_text() {
        let help = "Run it\n\nUsage: burrow [OPTIONS] <COMMAND>\n\nCommands:\n  server          Run the public server\n  client-config   Print a ready client config\n                  that wraps\n  help            Print this message\n\nOptions:\n  -h, --help  Print help\n";
        assert_eq!(commands_in(help), ["server", "client-config", "help"]);
        assert!(commands_in("Usage: burrow gensecret\n\nOptions:\n  -h\n").is_empty());
    }

    #[test]
    fn a_rendered_file_parses_back() {
        let mut s = Sections::new();
        s.insert("help burrow".into(), v(&["Usage: burrow", "", "  -h"]));
        s.insert("sizes".into(), v(&["release-binary 10"]));
        s.insert("fuzz targets".into(), Vec::new());
        assert_eq!(parse(&render(&s)), s);
    }

    #[test]
    fn tests_may_only_gain_or_move_module() {
        let old = v(&["[. lib burrow] service::tests::a", "[. test x] b"]);
        let moved = v(&[
            "[. lib burrow] service::install::tests::a",
            "[. test x] b",
            "[. test x] c",
        ]);
        assert!(lost_tests(&old, &moved).is_empty());
        let renamed = v(&["[. lib burrow] service::tests::a2", "[. test x] b"]);
        assert_eq!(
            lost_tests(&old, &renamed),
            ["[. lib burrow] service::tests::a"]
        );
        let other_binary = v(&["[. test y] service::tests::a", "[. test x] b"]);
        assert_eq!(lost_tests(&old, &other_binary).len(), 1);
        let twice = v(&["[. test x] m::a", "[. test x] n::a"]);
        assert_eq!(lost_tests(&twice, &v(&["[. test x] k::a"])).len(), 1);
    }

    #[test]
    fn sizes_may_move_one_percent() {
        let old = v(&["release-binary 1000", "wasm-gzip 500"]);
        assert!(size_problems(&old, &v(&["release-binary 1010", "wasm-gzip 495"])).is_empty());
        assert_eq!(
            size_problems(&old, &v(&["release-binary 1011", "wasm-gzip 500"])).len(),
            1
        );
        assert_eq!(size_problems(&old, &v(&["release-binary 1000"])).len(), 1);
    }

    #[test]
    fn help_schema_and_fuzz_must_be_identical() {
        let mut old = Sections::new();
        old.insert("help burrow".into(), v(&["a"]));
        old.insert("fuzz targets".into(), v(&["bin x"]));
        old.insert("public items".into(), v(&["mod burrow::a"]));
        old.insert("tests default".into(), v(&["[k] a"]));
        let mut new = old.clone();
        assert!(compare(&old, &new).problems.is_empty());
        new.insert("public items".into(), v(&["mod burrow::b"]));
        new.insert("tests default".into(), v(&["[k] a", "[k] b"]));
        let r = compare(&old, &new);
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert!(!r.notes.is_empty());
        new.insert("help burrow".into(), v(&["b"]));
        new.insert("help burrow x".into(), v(&["c"]));
        new.remove("fuzz targets");
        let r = compare(&old, &new);
        let heads: Vec<&String> = r.problems.iter().filter(|p| !p.starts_with("  ")).collect();
        assert_eq!(heads.len(), 3, "{heads:?}");
    }

    #[test]
    fn fuzz_targets_come_from_the_manifest_and_the_folder() {
        let manifest = "[package]\nname = \"burrow-fuzz\"\n\n[[bin]]\nname = \"frames\"\npath = \"fuzz_targets/frames.rs\"\n\n[[bin]]\nname = \"config\"\n";
        assert_eq!(
            fuzz_targets(manifest, &v(&["frames.rs", "config.rs"])),
            [
                "bin config",
                "bin frames",
                "file config.rs",
                "file frames.rs"
            ]
        );
    }

    #[test]
    fn the_fixture_is_hashed_with_its_schema_hash() {
        let a = fixture_lines("{\"hash\": \"b2s:1\"}\n");
        assert_eq!(a[1], "schema hash b2s:1");
        assert_eq!(a, fixture_lines("{\"hash\": \"b2s:1\"}\r\n"));
        assert_ne!(a[0], fixture_lines("{\"hash\": \"b2s:2\"}\n")[0]);
    }

    #[test]
    fn test_binaries_and_names_come_from_cargo() {
        let root = Path::new("/r");
        let json = [
            r#"{"reason":"compiler-artifact","profile":{"test":true},"executable":"/r/target/debug/deps/x-1","target":{"kind":["test"],"name":"x"},"manifest_path":"/r/Cargo.toml"}"#,
            r#"{"reason":"compiler-artifact","profile":{"test":true},"executable":"/r/target/debug/deps/c-1","target":{"kind":["lib"],"name":"c"},"manifest_path":"/r/crates/c/Cargo.toml"}"#,
            r#"{"reason":"compiler-artifact","profile":{"test":true},"executable":"/r/target/debug/deps/b-1","target":{"kind":["bench"],"name":"b"},"manifest_path":"/r/Cargo.toml"}"#,
            r#"{"reason":"compiler-artifact","profile":{"test":false},"executable":"/r/target/debug/burrow","target":{"kind":["bin"],"name":"burrow"},"manifest_path":"/r/Cargo.toml"}"#,
            r#"{"reason":"build-finished","success":true}"#,
        ]
        .join("\n");
        let keys: Vec<String> = test_binaries(root, &json)
            .into_iter()
            .map(|b| b.key)
            .collect();
        assert_eq!(keys, [". test x", "crates/c lib c"]);
        assert_eq!(
            listed(". test x", "a::b: test\nc: test\n\n2 tests, 0 benchmarks\n"),
            ["[. test x] a::b", "[. test x] c"]
        );
    }

    #[test]
    fn public_items_are_the_local_crate_paths() {
        let doc = serde_json::json!({"paths": {
            "1": {"crate_id": 0, "path": ["burrow", "cli", "Cli"], "kind": "struct"},
            "2": {"crate_id": 3, "path": ["std", "vec", "Vec"], "kind": "struct"},
            "3": {"crate_id": 0, "path": ["burrow", "cli"], "kind": "module"}
        }});
        assert_eq!(
            public_items(&doc),
            ["module burrow::cli", "struct burrow::cli::Cli"]
        );
    }
}
