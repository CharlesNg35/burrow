use burrow_admin_proto::msg::{AdminLevel, HistoryRequest};
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::value::{SecretSrc, as_secret};
use serde_json::json;

use burrow_config::{ServerConfig, ServiceEntry, WebConfig, WebSite};

use super::{World, apply_now, core_with_env, session, world};
use crate::admin::core::{Core, plan, text};

const LITERALS: [&str; 3] = ["c2VjcmV0LXZhbHVl", "live_key_lab_123", "sid=abc"];
const RESOLVED: &str = "env-token-value";

fn with_headers(w: &World) -> String {
    format!(
        "{}  site:\n    client: home\n    target: 3000\n    web:\n      hosts: [site.example.com]\n      tls: \"off\"\n      request_headers:\n        Authorization: Basic c2VjcmV0LXZhbHVl\n        X-Api-Key: live_key_lab_123\n        X-Env-Token: ${{UP_TOKEN}}\n        X-Frame-Options: DENY\nweb:\n  http: 7899\n  response_headers: {{Set-Cookie: \"sid=abc\", Server: \"\"}}\n",
        w.text
    )
}

fn site_headers() -> Vec<String> {
    let mut path = ServiceEntry::WEB.of("site");
    path.push(WebSite::REQUEST_HEADERS.key().into());
    path
}

fn answer_headers() -> Vec<String> {
    ServerConfig::WEB.then(WebConfig::RESPONSE_HEADERS).path()
}

fn header(mut path: Vec<String>, name: &str) -> Vec<String> {
    path.push(name.into());
    path
}

fn value_at<'a>(v: &'a serde_json::Value, path: &[String]) -> &'a serde_json::Value {
    burrow_admin_proto::value::get(v, path).expect("present")
}

fn core(w: &World) -> Core {
    let text = with_headers(w);
    let running = text.replace("${UP_TOKEN}", RESOLVED);
    core_with_env(&text, &running, &[("UP_TOKEN", RESOLVED)])
}

fn clear_of(text: &str) {
    for v in LITERALS.iter().chain([&RESOLVED]) {
        assert!(!text.contains(v), "{v} shows in {text}");
    }
}

#[test]
fn a_read_admin_sees_credential_headers_by_name_only() {
    let w = world();
    let core = core(&w);
    let state = core.state(&session(&w, AdminLevel::Read)).unwrap();
    clear_of(&serde_json::to_string(&state).unwrap());
    let headers = value_at(&state.values, &site_headers());
    assert_eq!(headers["X-Frame-Options"], "DENY");
    assert!(as_secret(&headers["Authorization"]).is_some(), "{headers}");
    assert!(as_secret(&headers["X-Api-Key"]).is_some(), "{headers}");
    let env = as_secret(&headers["X-Env-Token"]).unwrap();
    assert_eq!(env.src, SecretSrc::Env("UP_TOKEN".into()));
    assert_eq!(
        burrow_admin_proto::value::display(&headers["X-Env-Token"]),
        "${UP_TOKEN}"
    );
    let answers = value_at(&state.values, &answer_headers());
    assert!(as_secret(&answers["Set-Cookie"]).is_some(), "{answers}");
    assert_eq!(answers["Server"], "");
}

#[test]
fn plan_diffs_and_summaries_hide_credential_header_values() {
    let w = world();
    let core = core(&w);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    let ops = [
        Op::Set {
            path: header(site_headers(), "X-Api-Key"),
            value: json!("live_key_lab_456"),
        },
        Op::Set {
            path: header(answer_headers(), "Set-Cookie"),
            value: json!("sid=new"),
        },
    ];
    let c = plan::compute(&core, &s, &base, &ops).unwrap();
    let shown = serde_json::to_string(&c.plan).unwrap();
    clear_of(&shown);
    for v in ["live_key_lab_456", "sid=new"] {
        assert!(!shown.contains(v), "{v} shows in {shown}");
    }
    assert!(
        c.plan.diff.unified.contains("X-Api-Key: «hidden»"),
        "{}",
        c.plan.diff.unified
    );
    assert!(c.plan.diff.unified.contains("X-Frame-Options: DENY"));
    assert!(
        c.plan
            .changes
            .iter()
            .any(|ch| ch.summary.contains("«hidden, changed»")),
        "{:?}",
        c.plan.changes
    );
}

#[test]
fn the_history_diff_hides_credential_header_values() {
    let w = world();
    let core = core(&w);
    let s = session(&w, AdminLevel::Write);
    apply_now(
        &core,
        &s,
        &[Op::Set {
            path: header(site_headers(), "Authorization"),
            value: json!("Bearer fresh-upstream-credential"),
        }],
        &[],
    )
    .unwrap();
    let history = |entry| {
        core.history(&HistoryRequest {
            limit: None,
            diff: None,
            entry,
        })
        .unwrap()
    };
    let id = history(None).entries[0].id;
    let reply = history(Some(id));
    let shown = serde_json::to_string(&reply).unwrap();
    clear_of(&shown);
    assert!(!shown.contains("fresh-upstream-credential"), "{shown}");
    assert!(
        reply
            .diff
            .is_some_and(|d| d.unified.contains("Authorization: «hidden»"))
    );
}
