//! Parity of the editor's JSON Schema with burrow's own check, through `jsonschema`, the editor's validator.
//! `corpus` gathers the configs of the docs, `generate` makes random ones; `Verdict` compares both checks.
//! A schema problem is fine where burrow has an error at that field or a key inside it; `specific` picks
//! the problems the editor shows (config-editor-leptos `specific_errors`). Without the web proxy, burrow's
//! one `no-web-proxy` error stands for every `web:` key, and the editor refuses such a key whole.

#![cfg(test)]

mod corpus;
mod generate;

use std::collections::BTreeMap;

use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::text_edit::placeholders_in;
use jsonschema::error::{ValidationError, ValidationErrorKind};
use serde_json::Value;

use super::text_edit::edit_text;
use super::{core_running, session, world};
use crate::admin::core::Core;
use crate::admin::core::diags::Mapper;
use crate::admin::core::text_edit::editor_schema;
use crate::config::{Format, Role, load_str, tree};

const STRUCTURAL: [&str; 7] = [
    "unknown-field",
    "wrong-type",
    "unknown-value",
    "missing-field",
    "out-of-range",
    "not-a-mapping",
    "certificate-entry",
];

pub(super) struct Checks {
    validator: jsonschema::Validator,
    core: Core,
}

#[derive(Debug, Default)]
pub(super) struct Verdict {
    pub(super) false_errors: Vec<String>,
    pub(super) burrow: Vec<(Vec<String>, String)>,
    pub(super) editor: Vec<(Vec<String>, String)>,
}

const NO_WEB_PROXY: &str = "no-web-proxy";

fn site_keys(doc: Option<&tree::Node>) -> impl Iterator<Item = Vec<String>> + '_ {
    use burrow_config::{Entry, ServiceEntry};
    let section = ServiceEntry::section();
    let services = doc
        .and_then(|d| d.get(section))
        .and_then(|s| match &s.value {
            tree::Value::Map(entries) => Some(entries),
            _ => None,
        });
    services
        .into_iter()
        .flatten()
        .filter(|e| e.value.get(ServiceEntry::WEB.key()).is_some())
        .map(move |e| {
            vec![
                section.to_string(),
                e.key.clone(),
                ServiceEntry::WEB.key().to_string(),
            ]
        })
}

fn refused_block(at: &[String], message: &str, path: &[String]) -> bool {
    path.starts_with(at)
        && path
            .get(at.len())
            .is_some_and(|key| message.contains(&format!("'{key}' was unexpected")))
}

fn near(p: &[String], q: &[String]) -> bool {
    q == p || (q.len() == p.len() + 1 && q.starts_with(p))
}

impl Verdict {
    fn structural(&self) -> bool {
        self.burrow
            .iter()
            .any(|(_, c)| STRUCTURAL.contains(&c.as_str()))
    }
}

fn specific<'e, 'a>(error: &'e ValidationError<'a>) -> Vec<&'e ValidationError<'a>> {
    let branches = match error.kind() {
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            context
        }
        _ => return vec![error],
    };
    let path = error.instance_path().as_str();
    let wrong_type = |branch: &Vec<ValidationError<'a>>| {
        branch.iter().all(|nested| {
            matches!(nested.kind(), ValidationErrorKind::Type { .. })
                && nested.instance_path().as_str() == path
        })
    };
    let depth = |e: &ValidationError<'_>| e.instance_path().as_str().matches('/').count();
    let deepest_in = |branch: &'e Vec<ValidationError<'a>>| {
        branch
            .iter()
            .flat_map(specific)
            .map(&depth)
            .max()
            .unwrap_or(0)
    };
    let meant = branches.iter().filter(|b| !wrong_type(b));
    if let Some(branch) = meant.max_by_key(|b| deepest_in(b)) {
        return branch.iter().flat_map(specific).collect();
    }
    match branches
        .iter()
        .flatten()
        .flat_map(specific)
        .max_by_key(|e| depth(e))
    {
        Some(deepest) if depth(deepest) > depth(error) => vec![deepest],
        _ => vec![error],
    }
}

fn pointer(p: &str) -> Vec<String> {
    p.split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}

fn placeholder_pointers(v: &Value, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match v {
        Value::String(s)
            if placeholders_in(s)
                .first()
                .is_some_and(|(r, _)| *r == (0..s.len())) =>
        {
            out.push(at.clone())
        }
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            at.push(k.clone());
            placeholder_pointers(x, at, out);
            at.pop();
        }),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| {
            at.push(i.to_string());
            placeholder_pointers(x, at, out);
            at.pop();
        }),
        _ => {}
    }
}

impl Checks {
    pub(super) fn new() -> Checks {
        let w = world();
        let core = core_running(&w.text, &w.text);
        let schema = editor_schema(&core).clone();
        Checks {
            validator: jsonschema::validator_for(&schema).expect("the editor schema compiles"),
            core,
        }
    }

    pub(super) fn schema(&self) -> &Value {
        editor_schema(&self.core)
    }

    pub(super) fn editor(&self, text: &str) -> Vec<(Vec<String>, String)> {
        let Ok(doc) = tree::parse(text) else {
            return vec![(Vec::new(), "does not parse".into())];
        };
        let json = doc.to_json();
        let mut accepted = Vec::new();
        placeholder_pointers(&json, &mut Vec::new(), &mut accepted);
        let tops: Vec<ValidationError> = self.validator.iter_errors(&json).collect();
        tops.iter()
            .flat_map(specific)
            .map(|e| (pointer(e.instance_path().as_str()), e.to_string()))
            .filter(|(p, _)| !accepted.iter().any(|a| p.starts_with(a)))
            .collect()
    }

    pub(super) fn verdict(&self, text: &str, env: &BTreeMap<String, String>) -> Verdict {
        let lookup = |k: &str| env.get(k).cloned();
        let o = load_str(
            "server.yaml",
            text,
            Format::Yaml,
            Some(Role::Server),
            &lookup,
        );
        let doc = tree::parse(text).ok();
        let mapper = Mapper::new(doc.as_ref());
        let mut errors: Vec<(Vec<String>, String)> = o
            .report
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .map(|d| {
                let shown = mapper.convert(d);
                (shown.path.unwrap_or_default(), shown.code)
            })
            .collect();
        if errors.iter().any(|(_, c)| c == NO_WEB_PROXY) {
            errors.extend(site_keys(doc.as_ref()).map(|p| (p, NO_WEB_PROXY.to_string())));
        }
        let editor = self.editor(text);
        let excused = |p: &[String]| errors.iter().any(|(q, _)| near(p, q));
        Verdict {
            false_errors: editor
                .iter()
                .filter(|(p, _)| !excused(p))
                .map(|(p, m)| format!("/{}: {m}", p.join("/")))
                .collect(),
            burrow: errors,
            editor,
        }
    }

    pub(super) fn placeholders_only(&self, text: &str) -> Option<String> {
        let w = world();
        let core = core_running(text, &w.text);
        let masked = edit_text(&core, &session(&w, AdminLevel::Read)).files[0]
            .text
            .clone();
        let before = self.editor(text);
        let left: Vec<_> = self
            .editor(&masked)
            .into_iter()
            .filter(|(p, _)| !before.iter().any(|(q, _)| q == p))
            .collect();
        (!left.is_empty()).then(|| format!("masked:\n{masked}=> {left:#?}"))
    }
}

fn report(kind: &str, broken: &[String]) {
    assert!(
        broken.is_empty(),
        "{} {kind} configs that burrow accepts get schema problems:\n{}",
        broken.len(),
        broken.join("\n\n")
    );
}

#[test]
fn every_config_in_the_docs_validates_as_written_and_with_placeholders() {
    let checks = Checks::new();
    let env = corpus::env();
    let mut broken = Vec::new();
    let all = corpus::configs();
    assert!(all.len() >= 60, "only {} configs found", all.len());
    for (name, text) in &all {
        let v = checks.verdict(text, &env);
        if !v.false_errors.is_empty() {
            broken.push(format!("{name}:\n{text}=> {:#?}", v.false_errors));
        }
        broken.extend(
            checks
                .placeholders_only(text)
                .map(|m| format!("{name} {m}")),
        );
    }
    report("docs", &broken);
}

#[test]
fn random_configs_burrow_accepts_validate_and_its_refusals_are_problems() {
    let checks = Checks::new();
    let source = crate::config::load::schema_json(Role::Server);
    let mut broken = Vec::new();
    let (mut accepted, mut refused, mut env_used) = (0, 0, 0);
    for seed in 0..generate::CASES {
        let g = generate::config(&source, seed);
        for (variant, text, env) in [
            ("as written", g.text.clone(), BTreeMap::new()),
            ("with variables", g.with_env.clone(), g.env.clone()),
        ] {
            let v = checks.verdict(&text, &env);
            if !v.structural() {
                accepted += 1;
                env_used += usize::from(!env.is_empty());
            }
            if !v.false_errors.is_empty() {
                broken.push(format!(
                    "seed {seed} {variant}:\n{text}=> {:#?}",
                    v.false_errors
                ));
            }
        }
        if let Some((what, text, path)) = generate::refusal(&g, seed) {
            let v = checks.verdict(&text, &BTreeMap::new());
            let burrow = v
                .burrow
                .iter()
                .any(|(q, c)| q == &path && STRUCTURAL.contains(&c.as_str()));
            if burrow {
                refused += 1;
                if !v
                    .editor
                    .iter()
                    .any(|(p, m)| near(p, &path) || refused_block(p, m, &path))
                {
                    broken.push(format!(
                        "seed {seed}: {what} at {path:?} is refused by burrow only:\n{text}"
                    ));
                }
            }
        }
    }
    report("random", &broken);
    assert!(
        accepted >= generate::CASES / 2,
        "only {accepted} random configs have the right shape"
    );
    assert!(
        env_used >= generate::CASES / 4,
        "only {env_used} of them use variables"
    );
    assert!(refused >= generate::CASES / 2, "only {refused} refusals");
}

#[test]
fn a_variable_may_stand_for_any_value_and_a_typo_is_a_problem() {
    let checks = Checks::new();
    let ok = "listen: ${PORT}\nlimits:\n  max_pending: ${PENDING}\ninsecure_plaintext: ${PLAIN}\nservices:\n  web:\n    client: home\n    target: 3000\n    mode: ${MODE}\n";
    assert_eq!(checks.editor(ok), Vec::new());
    for bad in [
        "limits:\n  max_pending: lots\n",
        "insecure_plaintext: maybe\n",
        "services:\n  web:\n    client: home\n    target: 3000\n    mode: random\n",
        "listen: 7835\nlistne: 7836\n",
        "limits:\n  max_pending: $PENDING\n",
    ] {
        assert!(!checks.editor(bad).is_empty(), "{bad}");
    }
}

#[cfg(feature = "web")]
#[test]
fn the_schema_names_the_options_of_each_one_of_group_and_marks_locks_and_secrets() {
    let checks = Checks::new();
    let s = checks.schema();
    let dns = &s["properties"]["web"]["anyOf"][0]["properties"]["letsencrypt"]["anyOf"][0]["properties"]
        ["dns"]["items"];
    let options = dns["oneOf"].as_array().unwrap_or_else(|| panic!("{dns:#}"));
    let required: Vec<&Value> = options.iter().map(|o| &o["required"][0]).collect();
    assert_eq!(required, ["cloudflare", "rfc2136", "acme_dns", "command"]);
    assert!(options.iter().all(|o| o["title"].is_string()));
    assert_eq!(dns["properties"]["command"]["readOnly"], true);
    assert!(dns["properties"]["command"]["x-readonly-reason"].is_string());
    let token = &dns["properties"]["cloudflare"]["anyOf"][0]["properties"]["token"];
    assert_eq!(token["x-secret"], true, "{token:#}");
    let client = &s["properties"]["clients"]["additionalProperties"];
    assert_eq!(client["properties"]["secret"]["x-secret"], true);
    assert!(client["properties"]["secret"]["title"].is_string());
    assert!(s.get("$defs").is_none() && !s.to_string().contains("$ref"));
    let two_ca = "web:\n  letsencrypt:\n    email: a@example.com\n    ca: ca.pem\n    ca_upload: ca-1a2b3c4d5e6f7a8b\n";
    assert!(!checks.editor(two_ca).is_empty());
    let two_providers = "web:\n  letsencrypt:\n    email: a@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: { token: x }\n        command: /bin/true\n";
    assert!(!checks.editor(two_providers).is_empty());
    let null_provider = "web:\n  letsencrypt:\n    email: a@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare:\n        command: /bin/true\n";
    assert_eq!(checks.editor(null_provider), Vec::new());
}

#[cfg(feature = "web")]
#[test]
fn each_choice_of_a_pick_one_group_alone_is_fine() {
    let checks = Checks::new();
    let le = "web:\n  https: 443\n  letsencrypt:\n    email: a@example.com\n";
    let site = "services:\n  web:\n    client: home\n    target: 3000\n    web:\n      hosts: [a.example.com]\n";
    for text in [
        format!("{le}    ca: ca.pem\n"),
        format!("{le}    ca_upload: ca-1a2b3c4d5e6f7a8b\n"),
        le.to_string(),
        format!("{le}    dns:\n      - domains: [a.com]\n        cloudflare: {{ token: x }}\n"),
        format!("{le}    dns:\n      - domains: [a.com]\n        rfc2136: {{ server: ns, key_name: k, secret: s }}\n"),
        format!("{le}    dns:\n      - domains: [a.com]\n        acme_dns: {{ api: https://a, username: u, password: p, subdomain: s }}\n"),
        format!("{le}    dns:\n      - domains: [a.com]\n        command: /bin/true\n"),
        "web:\n  certificates:\n    - { hosts: [a.com], cert: a.pem, key: a.key }\n    - { hosts: [b.com], upload: pair-1a2b3c4d5e6f7a8b }\n".to_string(),
        format!("{site}      tls: {{ cert: a.pem, key: a.key }}\n      target_tls: {{ ca: ca.pem }}\n"),
        format!("{site}      tls: {{ upload: pair-1a2b3c4d5e6f7a8b }}\n      target_tls: {{ ca_upload: ca-1a2b3c4d5e6f7a8b }}\n"),
        format!("{site}      client_auth: {{ ca: ca.pem, crl_upload: crl-1a2b3c4d5e6f7a8b }}\n"),
        format!("{site}      client_auth: {{ ca_upload: ca-1a2b3c4d5e6f7a8b }}\n"),
    ] {
        assert_eq!(checks.editor(&text), Vec::new(), "{text}");
    }
    for text in [
        "web:\n  certificates:\n    - { hosts: [a.com], cert: a.pem }\n".to_string(),
        "web:\n  certificates:\n    - { hosts: [a.com], cert: a.pem, key: a.key, upload: pair-1a2b3c4d5e6f7a8b }\n".to_string(),
        format!("{site}      client_auth: {{ ca: ca.pem, ca_upload: ca-1a2b3c4d5e6f7a8b }}\n"),
        format!("{le}    dns:\n      - domains: [a.com]\n"),
    ] {
        assert!(!checks.editor(&text).is_empty(), "{text}");
    }
}

#[test]
fn the_schema_offers_web_settings_only_when_the_build_has_the_web_proxy() {
    let checks = Checks::new();
    let s = checks.schema();
    let service = &s["properties"]["services"]["additionalProperties"]["properties"];
    let with_web = crate::base::build::WEB_PROXY;
    assert_eq!(s["properties"].get("web").is_some(), with_web);
    assert_eq!(
        service["web"].get("type") == Some(&serde_json::json!("null")),
        !with_web
    );
    let unset = "services:\n  web:\n    client: home\n    target: 3000\n    web:\n";
    assert_eq!(checks.editor(unset), Vec::new());
    let site = "services:\n  web:\n    client: home\n    target: 3000\n    web:\n      hosts: [a.example.com]\n";
    let proxy = "web:\n  https: 443\n";
    for text in [site, proxy] {
        assert_eq!(checks.editor(text).is_empty(), with_web, "{text}");
        let v = checks.verdict(text, &BTreeMap::new());
        assert!(v.false_errors.is_empty(), "{text}: {:?}", v.false_errors);
    }
}
