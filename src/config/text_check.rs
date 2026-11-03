use std::path::Path;

use burrow_admin_proto::check::{CheckRole, CheckScope, READ_WHERE_BURROW_RUNS, WHOLE_FILE_ONLY};
use serde_json::Value;

use crate::config::diagnostics::{Diagnostic, Report, mask_text};
use crate::config::env::EnvSource;
use crate::config::{Format, Outcome, Role};

pub const LITERAL_MIN: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reads {
    Here,
    Elsewhere,
}

pub fn role(r: CheckRole) -> Role {
    match r {
        CheckRole::Server => Role::Server,
        CheckRole::Client => Role::Client,
        CheckRole::Visitor => Role::Visitor,
    }
}

pub fn format_of(text: &str) -> Format {
    if text.trim_start().starts_with('{') {
        Format::Json
    } else {
        Format::Yaml
    }
}

pub fn load(
    text: &str,
    role: Role,
    (dir, state): (&Path, Option<&Path>),
    name: &str,
    env: &dyn EnvSource,
) -> Outcome {
    crate::config::load::load_str_at((dir, state), name, text, format_of(text), Some(role), env)
}

fn kept(d: &Diagnostic, scope: CheckScope, reads: Reads) -> bool {
    let elsewhere = reads == Reads::Elsewhere && READ_WHERE_BURROW_RUNS.contains(&d.code);
    let whole = scope == CheckScope::Fragment && WHOLE_FILE_ONLY.contains(&d.code);
    !elsewhere && !whole
}

fn literals(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || "+/=-_".contains(c)))
        .filter(|t| t.len() >= LITERAL_MIN)
        .map(String::from)
        .collect()
}

fn masked(v: &mut Value, hidden: &[String]) {
    match v {
        Value::String(s) => {
            let mut text = mask_text(s);
            for h in hidden {
                text = text.replace(h.as_str(), &"*".repeat(h.chars().count()));
            }
            *s = text;
        }
        Value::Array(a) => a.iter_mut().for_each(|x| masked(x, hidden)),
        Value::Object(o) => o.values_mut().for_each(|x| masked(x, hidden)),
        _ => {}
    }
}

pub fn report_json(
    mut report: Report,
    text: &str,
    scope: CheckScope,
    reads: Reads,
    values: &[String],
) -> Value {
    report.diagnostics.retain(|d| kept(d, scope, reads));
    let mut v = report.render_json();
    if let Some(items) = v["diagnostics"].as_array_mut() {
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            item.remove("file");
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("file");
    }
    let names = crate::config::env::referenced_env(text);
    let mut hidden: Vec<String> = literals(text)
        .into_iter()
        .filter(|l| !names.contains(l))
        .chain(values.iter().filter(|s| s.len() >= 4).cloned())
        .collect();
    hidden.sort_by_key(|t| std::cmp::Reverse(t.len()));
    hidden.dedup();
    masked(&mut v, &hidden);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::codes;

    fn checked(text: &str, scope: CheckScope, reads: Reads, env: &dyn EnvSource) -> Value {
        let o = load(
            text,
            Role::Server,
            (Path::new("."), None),
            "config.yaml",
            env,
        );
        let values: Vec<String> = crate::config::env::referenced_env(text)
            .iter()
            .filter_map(|n| env.get(n))
            .collect();
        report_json(o.report, text, scope, reads, &values)
    }

    fn codes(v: &Value) -> Vec<String> {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_fragment_drops_only_whole_file_findings_and_elsewhere_drops_unread_values() {
        let none = |_: &str| None::<String>;
        let text = "listen: ${NOT_SET_FOR_BURROW}\n";
        let here = codes(&checked(text, CheckScope::Whole, Reads::Here, &none));
        assert!(here.contains(&codes::ENV_UNSET.to_string()), "{here:?}");
        let away = codes(&checked(
            text,
            CheckScope::Fragment,
            Reads::Elsewhere,
            &none,
        ));
        assert!(!away.contains(&codes::ENV_UNSET.to_string()), "{away:?}");
        let bare = "listen: 127.0.0.1:7835\n";
        let whole = codes(&checked(bare, CheckScope::Whole, Reads::Here, &none));
        assert!(whole.contains(&codes::NO_CLIENTS.to_string()), "{whole:?}");
        let part = codes(&checked(bare, CheckScope::Fragment, Reads::Here, &none));
        assert!(!part.contains(&codes::NO_CLIENTS.to_string()), "{part:?}");
    }

    #[test]
    fn values_of_variables_secrets_and_long_literals_are_never_echoed() {
        let env = |n: &str| (n == "LISTEN_AT").then(|| "my-hidden-value".to_string());
        let v = checked(
            "listen: ${LISTEN_AT}\n",
            CheckScope::Whole,
            Reads::Here,
            &env,
        );
        assert_eq!(v["valid"], false, "{v}");
        assert!(!v.to_string().contains("my-hidden-value"), "{v}");
        let secret = crate::wire::secret::Secret::generate().encode();
        let none = |_: &str| None::<String>;
        let v = checked(
            &format!("listen: {secret}\n"),
            CheckScope::Whole,
            Reads::Here,
            &none,
        );
        assert!(!v.to_string().contains(&secret), "{v}");
        let token = "cf_0123456789abcdefghijklmnop";
        let v = checked(
            &format!("listen: {token}\n"),
            CheckScope::Whole,
            Reads::Here,
            &none,
        );
        assert!(!v.to_string().contains(token), "{v}");
        assert!(v.get("file").is_none() && v["diagnostics"][0].get("file").is_none());
    }
}
