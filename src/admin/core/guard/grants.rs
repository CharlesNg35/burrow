use burrow_admin_proto::plan::Refusal;
use burrow_syntax::grant::{Grant, Power, grants};
use serde_json::Value;

use super::{Base, Change, refusal};
use crate::config::model::AdminLevel;
use crate::config::settings::{ServerSettings, VisitorAuth};
use crate::secret::Secret;

fn power(v: &VisitorAuth) -> Power<AdminLevel, &Secret> {
    Power {
        level: v.admin,
        secrets: v.secrets.iter().collect(),
        from: v.admin_from.clone(),
    }
}

fn nothing() -> Power<AdminLevel, String> {
    Power {
        level: None,
        secrets: Vec::new(),
        from: Vec::new(),
    }
}

fn raw_power(v: &Value) -> Power<AdminLevel, String> {
    let level = match v.get("admin") {
        None | Some(Value::Null) => None,
        Some(x) => match serde_json::from_value(x.clone()) {
            Ok(l) => Some(l),
            Err(_) => return nothing(),
        },
    };
    let secrets = match v.get("secret") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    };
    let from = match v.get("admin_from") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => {
            let parsed: Vec<_> = a
                .iter()
                .filter_map(|x| x.as_str().and_then(|t| t.parse().ok()))
                .collect();
            if parsed.len() != a.len() {
                return nothing();
            }
            parsed
        }
        Some(_) => return nothing(),
    };
    Power {
        level,
        secrets,
        from,
    }
}

fn level(l: AdminLevel) -> &'static str {
    match l {
        AdminLevel::Read => "read",
        AdminLevel::Write => "write",
    }
}

fn what<S>(
    name: &str,
    renamed_from: Option<&str>,
    g: &Grant<AdminLevel, S>,
    fp: impl Fn(&S) -> String,
) -> String {
    match (g, renamed_from) {
        (Grant::Admin(_), Some(old)) => format!(
            "an admin visitor is renamed only in the config file: visitor \"{old}\" would become \"{name}\""
        ),
        (Grant::Admin(l), None) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would get admin: {}",
            level(*l)
        ),
        (Grant::Upgrade, _) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would go from admin: read to admin: write"
        ),
        (Grant::SecretAdded(s), _) => format!(
            "admin access is granted only in the config file: visitor \"{name}\" would get the new secret {}",
            fp(s)
        ),
        (Grant::FromWidened(Some(c)), _) => format!(
            "admin access is granted only in the config file: admin_from of visitor \"{name}\" would admit {c}, which the current list does not"
        ),
        (Grant::FromWidened(None), _) => format!(
            "admin access is granted only in the config file: admin_from of visitor \"{name}\" would be removed, which admits every address"
        ),
    }
}

fn raw_fp(s: &str) -> String {
    Secret::parse(s).map_or_else(|_| "(not decodable)".into(), |x| x.fingerprint())
}

pub fn refusals(c: &Change) -> Vec<Refusal> {
    let file = c.path.display();
    let renamed_from = |name: &str| {
        c.renames
            .iter()
            .find(|r| r.to == name && r.path.len() == 2 && r.path[0] == "visitors")
            .map(|r| r.path[1].clone())
    };
    let against = |old: &ServerSettings, out: &mut Vec<(&'static str, String)>| {
        for (name, v) in &c.next.visitors {
            let base = old.visitors.get(name).map(power);
            let new = power(v);
            let from = renamed_from(name);
            for g in grants(base.as_ref(), &new) {
                out.push((
                    g.code(),
                    what(name, from.as_deref(), &g, |s| s.fingerprint()),
                ));
            }
        }
    };
    let mut found = Vec::new();
    let tail = match c.base {
        Base::Valid(settings) => {
            against(settings, &mut found);
            format!("use burrow edit -c {file} or edit the file")
        }
        Base::Broken { raw, next_raw } => {
            against(c.running, &mut found);
            let visitors = |t: &Value| t.get("visitors").and_then(Value::as_object).cloned();
            let (old, new) = (
                visitors(raw).unwrap_or_default(),
                visitors(next_raw).unwrap_or_default(),
            );
            for (name, v) in &new {
                let base = old.get(name).map(raw_power);
                let next = raw_power(v);
                let from = renamed_from(name);
                for g in grants(base.as_ref(), &next) {
                    found.push((
                        g.code(),
                        what(name, from.as_deref(), &g, |s: &String| raw_fp(s)),
                    ));
                }
            }
            format!(
                "the config file has errors, so this change is checked against both the running config and the file; fix the errors and make admin changes in the file with burrow edit -c {file}"
            )
        }
    };
    let mut out: Vec<Refusal> = Vec::new();
    for (code, message) in found {
        let message = format!("{message}; {tail}");
        if !out.iter().any(|r| r.message == message) {
            out.push(refusal(code, message));
        }
    }
    out
}
