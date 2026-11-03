use std::path::Path;

use burrow_admin_proto::codes;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::diagnostics::{Diagnostic, mask_text};
use crate::config::{Format, Role};

pub const MAX_INPUT: usize = 64 * 1024;
pub const NAME: &str = "config.yaml";

const NOTE: &str = "checked on this computer by burrow's own validation, as `burrow validate` does; nothing was written or sent to the server. ${VAR} values and files named in the config are not read here: they are read where burrow runs.";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RoleWord {
    #[default]
    Server,
    Client,
    Visitor,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Fragment,
    Whole,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateArgs {
    #[schemars(
        description = "The config to check, as YAML (or JSON) text: a whole config file, or a fragment such as a web: section"
    )]
    pub config: String,
    #[schemars(description = "Which config it is: server (the default), client or visitor")]
    #[serde(default)]
    pub role: RoleWord,
    #[schemars(
        description = "fragment (the default): part of a larger config, so a missing listen, clients or services is not reported; whole: a complete file, as burrow validate checks it"
    )]
    #[serde(default)]
    pub scope: Scope,
}

fn role(r: RoleWord) -> Role {
    match r {
        RoleWord::Server => Role::Server,
        RoleWord::Client => Role::Client,
        RoleWord::Visitor => Role::Visitor,
    }
}

fn kept(d: &Diagnostic, scope: Scope) -> bool {
    let elsewhere = [codes::ENV_UNSET, codes::TLS_UNREADABLE];
    let whole_file = ["no-listener", codes::NO_CLIENTS, codes::NO_SERVICES];
    !elsewhere.contains(&d.code) && (scope == Scope::Whole || !whole_file.contains(&d.code))
}

pub const LITERAL_MIN: usize = 20;

fn literals(input: &str) -> Vec<String> {
    let mut found: Vec<String> = input
        .split(|c: char| !(c.is_ascii_alphanumeric() || "+/=-_".contains(c)))
        .filter(|t| t.len() >= LITERAL_MIN)
        .map(String::from)
        .collect();
    found.sort_by_key(|t| std::cmp::Reverse(t.len()));
    found.dedup();
    found
}

fn masked(v: &mut Value, hidden: &[String]) {
    match v {
        Value::String(s) => {
            let mut text = mask_text(s);
            for h in hidden {
                text = text.replace(h.as_str(), &"*".repeat(h.len()));
            }
            *s = text;
        }
        Value::Array(a) => a.iter_mut().for_each(|x| masked(x, hidden)),
        Value::Object(o) => o.values_mut().for_each(|x| masked(x, hidden)),
        _ => {}
    }
}

pub fn too_large(len: usize) -> Option<Value> {
    (len > MAX_INPUT).then(|| {
        json!({
            "error": "too-large",
            "message": format!("the config is {len} bytes; validate_config takes at most {MAX_INPUT}"),
            "hint": "check one section at a time",
        })
    })
}

pub fn check(a: &ValidateArgs) -> Value {
    if let Some(v) = too_large(a.config.len()) {
        return v;
    }
    let format = if a.config.trim_start().starts_with('{') {
        Format::Json
    } else {
        Format::Yaml
    };
    let none = |_: &str| None::<String>;
    let outcome = crate::config::load::load_str_in(
        Path::new("."),
        NAME,
        &a.config,
        format,
        Some(role(a.role)),
        &none,
    );
    let mut report = outcome.report;
    report.diagnostics.retain(|d| kept(d, a.scope));
    let mut v = report.render_json();
    if let Some(items) = v["diagnostics"].as_array_mut() {
        for item in items.iter_mut().filter_map(Value::as_object_mut) {
            item.remove("file");
        }
    }
    if let Some(o) = v.as_object_mut() {
        o.remove("file");
    }
    masked(&mut v, &literals(&a.config));
    v["note"] = NOTE.into();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "web")]
    const OWNER_EXAMPLE: &str = "web:
  letsencrypt:
    email: ops@example.com
    dns:
      - domains:
          - example.com
        cloudflare:
          token: ${CF_TOKEN}
        wildcards:
          - example.com
";

    const FLOW_UNQUOTED: &str = "web:
  letsencrypt:
    email: ops@example.com
    dns:
      - domains: [example.com]
        cloudflare: { token: ${CF_TOKEN} }
        wildcards: [\"*.example.com\"]
";

    fn of(config: &str, scope: Scope) -> Value {
        check(&ValidateArgs {
            config: config.into(),
            role: RoleWord::Server,
            scope,
        })
    }

    #[cfg(feature = "web")]
    fn messages(v: &Value) -> Vec<String> {
        v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["message"].as_str().unwrap().to_string())
            .collect()
    }

    #[cfg(feature = "web")]
    #[test]
    fn the_owner_s_example_is_refused_with_the_validate_message() {
        let v = of(OWNER_EXAMPLE, Scope::Fragment);
        assert_eq!(v["valid"], false, "{v}");
        let m = messages(&v);
        assert!(
            m.contains(&"web.letsencrypt.dns[0].wildcards: \"example.com\" is not a wildcard; write it as \"*.example.com\"".to_string()),
            "{v}"
        );
        assert!(!m.iter().any(|m| m.contains("CF_TOKEN")), "{v}");
        assert!(!m.iter().any(|m| m.contains("needs \"listen\"")), "{v}");
        let whole = messages(&of(OWNER_EXAMPLE, Scope::Whole));
        assert!(
            whole.iter().any(|m| m.contains("needs \"listen\"")),
            "{whole:?}"
        );
        let unquoted = of(FLOW_UNQUOTED, Scope::Fragment);
        let first = &unquoted["diagnostics"][0];
        assert!(
            first["help"].as_str().unwrap().contains("must be quoted"),
            "{unquoted}"
        );
        assert!(v["note"].as_str().unwrap().contains("nothing was written"));
    }

    #[cfg(feature = "web")]
    #[test]
    fn the_owner_s_example_with_a_real_wildcard_is_valid() {
        let fixed = OWNER_EXAMPLE.replace(
            "        wildcards:\n          - example.com\n",
            "        wildcards:\n          - \"*.example.com\"\n",
        );
        let v = of(&format!("{fixed}  https: 443\n"), Scope::Fragment);
        assert_eq!(v["valid"], true, "{v}");
    }

    #[test]
    fn secrets_and_long_values_are_never_echoed_and_the_size_is_capped() {
        let unquoted = of(FLOW_UNQUOTED, Scope::Fragment);
        assert_eq!(unquoted["diagnostics"][0]["code"], "syntax", "{unquoted}");
        let secret = crate::wire::secret::Secret::generate().encode();
        let leaky = format!("listen: {secret}\nclients:\n  home: {{ secret: {secret} }}\n");
        let v = of(&leaky, Scope::Whole);
        assert_eq!(v["valid"], false, "{v}");
        assert!(!v.to_string().contains(&secret), "{v}");
        let token = "cf_0123456789abcdefghijklmnop";
        let v = of(&format!("listen: {token}\n"), Scope::Whole);
        assert!(!v.to_string().contains(token), "{v}");
        let big = "#".repeat(MAX_INPUT + 1);
        assert_eq!(of(&big, Scope::Whole)["error"], "too-large");
    }
}
