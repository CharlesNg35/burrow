use std::path::Path;

use burrow_admin_proto::check::{CheckRole, CheckScope, CheckText};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::text_check::{self, Reads};

pub const NAME: &str = "config.yaml";
const ELSEWHERE: &str = "${VAR} values and files named in the config are not read here: they are read where burrow runs. Nothing was written or sent to the server.";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateArgs {
    #[schemars(
        description = "The config to check, as YAML (or JSON) text: a whole config file, or a fragment such as a web: section"
    )]
    pub config: String,
    #[schemars(description = "Which config it is: server (the default), client or visitor")]
    #[serde(default)]
    pub role: CheckRole,
    #[schemars(
        description = "fragment (the default): part of a larger config, so a missing listen, clients or services is not reported; whole: a complete file, as burrow validate checks it"
    )]
    #[serde(default)]
    pub scope: CheckScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    OlderServer(String),
    Unreachable(String),
}

pub fn too_large(len: usize) -> Option<Value> {
    burrow_admin_proto::check::too_large(len).map(|why| {
        json!({ "error": "too-large", "message": why, "hint": "check one section at a time" })
    })
}

pub fn request(a: &ValidateArgs) -> CheckText {
    CheckText {
        text: a.config.clone(),
        role: a.role,
        scope: a.scope,
    }
}

pub fn on_server(mut v: Value, server: &str) -> Value {
    v["checked_by"] = "server".into();
    v["note"] = format!(
        "checked by the server, which runs burrow {server}, with its own validation, environment variables and files; nothing was written"
    )
    .into();
    v
}

pub fn local(a: &ValidateArgs, why: &Local) -> Value {
    let none = |_: &str| None::<String>;
    let outcome = text_check::load(
        &a.config,
        text_check::role(a.role),
        (Path::new("."), None),
        NAME,
        &none,
    );
    let mut v = text_check::report_json(outcome.report, &a.config, a.scope, Reads::Elsewhere, &[]);
    let here = crate::base::build::version();
    let reason = match why {
        Local::OlderServer(server) => format!(
            "checked with burrow {here} on this computer; the server runs burrow {server} and may differ"
        ),
        Local::Unreachable(what) => format!(
            "checked with burrow {here} on this computer, because the server could not be reached ({what}); the server may differ"
        ),
    };
    v["checked_by"] = "local".into();
    v["note"] = format!("{reason}. {ELSEWHERE}").into();
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

    fn of(config: &str, scope: CheckScope) -> Value {
        let a = ValidateArgs {
            config: config.into(),
            role: CheckRole::Server,
            scope,
        };
        local(&a, &Local::OlderServer("0.1.2".into()))
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
        let v = of(OWNER_EXAMPLE, CheckScope::Fragment);
        assert_eq!(v["valid"], false, "{v}");
        let m = messages(&v);
        assert!(
            m.contains(&"web.letsencrypt.dns[0].wildcards: \"example.com\" is not a wildcard; write it as \"*.example.com\"".to_string()),
            "{v}"
        );
        assert!(!m.iter().any(|m| m.contains("CF_TOKEN")), "{v}");
        assert!(!m.iter().any(|m| m.contains("needs \"listen\"")), "{v}");
        let whole = messages(&of(OWNER_EXAMPLE, CheckScope::Whole));
        assert!(
            whole.iter().any(|m| m.contains("needs \"listen\"")),
            "{whole:?}"
        );
        let unquoted = of(FLOW_UNQUOTED, CheckScope::Fragment);
        let first = &unquoted["diagnostics"][0];
        assert!(
            first["help"].as_str().unwrap().contains("must be quoted"),
            "{unquoted}"
        );
        let note = v["note"].as_str().unwrap();
        assert!(
            note.contains("the server runs burrow 0.1.2 and may differ"),
            "{note}"
        );
        assert!(note.contains("Nothing was written"), "{note}");
        assert_eq!(v["checked_by"], "local");
    }

    #[cfg(feature = "web")]
    #[test]
    fn the_owner_s_example_with_a_real_wildcard_is_valid() {
        let fixed = OWNER_EXAMPLE.replace(
            "        wildcards:\n          - example.com\n",
            "        wildcards:\n          - \"*.example.com\"\n",
        );
        let v = of(&format!("{fixed}  https: 443\n"), CheckScope::Fragment);
        assert_eq!(v["valid"], true, "{v}");
    }

    #[test]
    fn secrets_and_long_values_are_never_echoed_and_the_size_is_capped() {
        let unquoted = of(FLOW_UNQUOTED, CheckScope::Fragment);
        assert_eq!(unquoted["diagnostics"][0]["code"], "syntax", "{unquoted}");
        let secret = crate::wire::secret::Secret::generate().encode();
        let leaky = format!("listen: {secret}\nclients:\n  home: {{ secret: {secret} }}\n");
        let v = of(&leaky, CheckScope::Whole);
        assert_eq!(v["valid"], false, "{v}");
        assert!(!v.to_string().contains(&secret), "{v}");
        let token = "cf_0123456789abcdefghijklmnop";
        let v = of(&format!("listen: {token}\n"), CheckScope::Whole);
        assert!(!v.to_string().contains(token), "{v}");
        let big = burrow_admin_proto::check::MAX_TEXT + 1;
        assert_eq!(too_large(big).unwrap()["error"], "too-large");
        let call = json!({ "arguments": { "config": "\n".repeat(big - 1) } }).to_string();
        assert!(call.len() <= crate::mcp::http::MAX_BODY, "{}", call.len());
        let a = ValidateArgs {
            config: "x: 1\n".into(),
            role: CheckRole::Server,
            scope: CheckScope::Fragment,
        };
        let away = local(&a, &Local::Unreachable("connection refused".into()));
        let note = away["note"].as_str().unwrap();
        assert!(
            note.contains("could not be reached (connection refused)"),
            "{note}"
        );
        let served = on_server(json!({ "valid": true }), "0.2.3");
        assert_eq!(served["checked_by"], "server");
        assert!(served["note"].as_str().unwrap().contains("burrow 0.2.3"));
    }
}
