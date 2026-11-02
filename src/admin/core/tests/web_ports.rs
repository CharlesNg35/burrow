#![cfg(test)]

use burrow_admin_proto::codes::{OPENS_PORT, PORT_IN_USE, STOPS_WEBSITES, WEB_PORT_SHARED};
use burrow_admin_proto::msg::AdminLevel;
use burrow_admin_proto::ops::Op;
use burrow_admin_proto::plan::Plan;
use serde_json::json;

use super::*;

fn with_web(web: &str) -> World {
    let mut w = world();
    w.text = w.text.replace(
        "\nservices:\n",
        &format!(
            "\n{web}\nservices:\n  blog: {{ client: home, target: 3000, web: {{ hosts: [blog.example.com], tls: off }} }}\n"
        ),
    );
    w
}

fn plan_of(core: &Core, w: &World, ops: Vec<Op>) -> Plan {
    let s = session(w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    match plan::compute(core, &s, &base, &ops) {
        Ok(c) => c.plan,
        Err(e) => panic!("{ops:?} was refused: {}", e.message),
    }
}

fn set(path: &[&str], value: serde_json::Value) -> Vec<Op> {
    vec![Op::Set {
        path: p(path),
        value,
    }]
}

fn impact<'a>(plan: &'a Plan, code: &str) -> Option<&'a str> {
    plan.impacts
        .iter()
        .find(|i| i.code == code)
        .map(|i| i.message.as_str())
}

const PORTS: &str = "web:\n  https: 8443\n  http: 8080\n";

#[test]
fn a_low_web_port_the_server_may_not_open_gets_one_warning_with_its_fix() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, vec![80]);
    let plan = plan_of(&core, &w, set(&["web", "https"], json!(80)));
    let at = |code: &str| plan.diags.iter().filter(|d| d.code == code).count();
    let addr: std::net::SocketAddr = "[::]:80".parse().unwrap();
    if !crate::config::check::low_port_refused(addr) {
        return;
    }
    assert_eq!(at(PORT_IN_USE), 0, "{:?}", plan.diags);
    let d = plan
        .diags
        .iter()
        .find(|d| d.code == "low-port")
        .expect("a low-port warning");
    assert!(!d.is_error());
    assert!(
        d.message.starts_with("web.https: port 80 needs root"),
        "{}",
        d.message
    );
    assert!(
        d.help
            .as_deref()
            .is_some_and(|h| h.contains(&crate::install::low_port_hint())),
        "{d:?}"
    );
}

#[test]
fn a_new_web_port_another_program_holds_refuses_the_plan_with_the_find_hint() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, vec![9443]);
    let plan = plan_of(&core, &w, set(&["web", "https"], json!(9443)));
    assert!(!plan.valid);
    let d = plan
        .diags
        .iter()
        .find(|d| d.code == PORT_IN_USE)
        .expect("a port-in-use error");
    assert!(d.is_error());
    assert_eq!(d.path, Some(p(&["web", "https"])));
    assert!(
        d.message.contains("address already in use"),
        "{}",
        d.message
    );
    let find = crate::base::ports::inspect_hint(burrow_config::Protocol::Tcp, 9443);
    assert!(
        d.help.as_deref().is_some_and(|h| h.contains(&find)),
        "{d:?}"
    );
}

#[test]
fn a_free_new_web_port_passes_and_review_says_it_opens() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, vec![9444]);
    let plan = plan_of(&core, &w, set(&["web", "https"], json!(9443)));
    assert!(plan.valid, "{:?}", plan.diags);
    assert_eq!(
        impact(&plan, OPENS_PORT),
        Some("opens port 9443 on all addresses for websites (web.https)")
    );
    let plan = plan_of(&core, &w, set(&["web", "http"], json!("127.0.0.1:8081")));
    assert_eq!(
        impact(&plan, OPENS_PORT),
        Some("opens port 8081 on 127.0.0.1 for websites (web.http)")
    );
}

#[test]
fn a_port_burrow_already_holds_is_not_probed() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, vec![8443, 8080]);
    let plan = plan_of(&core, &w, set(&["web", "idle_timeout_secs"], json!(30)));
    assert!(plan.valid, "{:?}", plan.diags);
    assert!(impact(&plan, OPENS_PORT).is_none());
    let plan = plan_of(&core, &w, set(&["web", "http"], json!(8081)));
    assert!(
        plan.valid,
        "the unchanged web.https is not tested: {:?}",
        plan.diags
    );
}

#[test]
fn a_new_web_block_is_probed_too() {
    let w = world();
    let core = core_busy(&w.text, &w.text, vec![9443]);
    let plan = plan_of(&core, &w, set(&["web"], json!({"https": 9443})));
    assert!(
        plan.diags.iter().any(|d| d.code == PORT_IN_USE),
        "{:?}",
        plan.diags
    );
    assert!(!plan.valid);
}

#[test]
fn removing_web_says_which_websites_stop_and_a_port_they_need_is_an_error() {
    let w = with_web(PORTS);
    let core = core_for(&w.text);
    let plan = plan_of(
        &core,
        &w,
        vec![
            Op::Remove { path: p(&["web"]) },
            Op::Remove {
                path: p(&["services", "blog"]),
            },
        ],
    );
    assert_eq!(impact(&plan, STOPS_WEBSITES), Some("stops websites blog"));
    let plan = plan_of(
        &core,
        &w,
        vec![Op::Remove {
            path: p(&["web", "http"]),
        }],
    );
    assert!(!plan.valid);
    assert!(
        plan.diags.iter().any(|d| d.code == "plain-needs-http"),
        "{:?}",
        plan.diags
    );
}

#[test]
fn a_web_port_shared_with_the_tunnel_stays_a_file_edit() {
    let w = with_web(
        "web:\n  https: 443\n  http: 8080\n  letsencrypt: { email: you@example.com }\nwebsocket: { listen: 443, host: tunnel.example.com }\n",
    );
    let core = core_for(&w.text);
    let s = session(&w, AdminLevel::Write);
    let base = text::rev(&core.read().unwrap());
    let ops = set(&["web", "https"], json!(8443));
    let plan = plan::compute(&core, &s, &base, &ops).unwrap().plan;
    let r = plan
        .refusals
        .iter()
        .find(|r| r.code == WEB_PORT_SHARED)
        .unwrap_or_else(|| panic!("not refused: {:?} {:?}", plan.refusals, plan.diags));
    assert!(
        r.message.contains("disconnects every client"),
        "{}",
        r.message
    );
    let plan = plan_of(&core, &w, set(&["web", "http"], json!(8081)));
    assert!(plan.refusals.is_empty(), "{:?}", plan.refusals);
}

#[test]
fn a_bind_on_an_address_this_server_lacks_is_a_warning_with_the_bind_hint() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, Vec::new());
    let plan = plan_of(
        &core,
        &w,
        set(&["services", "web", "bind"], json!("192.0.2.7:9000")),
    );
    let d = plan
        .diags
        .iter()
        .find(|d| d.code == burrow_admin_proto::codes::NOT_THIS_MACHINE)
        .expect("a not-this-machine warning");
    assert!(!d.is_error() && plan.valid, "{:?}", plan.diags);
    assert_eq!(d.path, Some(p(&["services", "web", "bind"])));
    assert_eq!(d.message, "services.web.bind: 192.0.2.7 is not an address of this server");
    assert_eq!(d.help, Some(crate::base::ports::not_here_hint()));
    let same = plan_of(&core, &w, set(&["services", "web", "bind"], json!("0.0.0.0:9000")));
    assert!(same.diags.iter().all(|d| d.code != burrow_admin_proto::codes::NOT_THIS_MACHINE));
}

#[test]
fn a_service_port_another_program_holds_is_refused_before_saving() {
    let w = with_web(PORTS);
    let core = core_busy(&w.text, &w.text, vec![9001, 9002]);
    let mut ops = set(&["services", "web", "bind"], json!("9000-9003"));
    ops.extend(set(&["services", "web", "target"], json!("3000-3003")));
    let plan = plan_of(&core, &w, ops);
    let found: Vec<_> = plan.diags.iter().filter(|d| d.code == PORT_IN_USE).collect();
    assert_eq!(found.len(), 1, "one error for the range: {:?}", plan.diags);
    assert!(!plan.valid);
    assert_eq!(found[0].path, Some(p(&["services", "web", "bind"])));
    assert!(found[0].message.contains("address already in use"), "{}", found[0].message);
    let same = plan_of(&core, &w, set(&["services", "web", "target"], json!(3001)));
    assert!(same.diags.iter().all(|d| d.code != PORT_IN_USE), "an unchanged bind is not probed");
}
