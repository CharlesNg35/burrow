#![cfg(test)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use serde_json::Value;

thread_local! {
    static LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn random_run(run: &str) -> bool {
    let hex = run.len() >= 8 && run.bytes().all(|b| b.is_ascii_hexdigit());
    let temp = run.len() == 9 && run.starts_with("tmp");
    hex || temp || crate::admin::core::text::hides_token(run)
}

fn canon(text: &str) -> String {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let mut flush = |run: &mut String, out: &mut String| {
        if random_run(run) {
            let n = seen.len();
            let id = *seen.entry(run.clone()).or_insert(n);
            out.push_str(&format!("#{id}"));
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-') {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

pub(super) fn note(what: &str, v: &Value) {
    if std::env::var_os("PROBE_DUMP").is_none() {
        return;
    }
    let digest = crate::admin::core::text::rev(&canon(&v.to_string()));
    LINES.with(|l| l.borrow_mut().push(format!("{what} {digest}")));
}

pub(super) fn flush(order: &str, seed: u64) {
    let Some(path) = std::env::var_os("PROBE_DUMP") else {
        return;
    };
    let lines = LINES.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let mut path = std::path::PathBuf::from(path).into_os_string();
    path.push(format!("-{order}"));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for (i, l) in lines.iter().enumerate() {
        writeln!(f, "{seed} {i} {l}").unwrap();
    }
}
