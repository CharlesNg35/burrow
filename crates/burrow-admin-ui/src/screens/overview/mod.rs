mod attention;
mod certs;
mod page;
mod tables;
mod web;

use burrow_admin_logic::rates::{axis_max, mbit};
use burrow_admin_proto::diag::Severity;
use burrow_admin_proto::msg::State;
use burrow_admin_proto::serde_json::Value;
use burrow_admin_proto::status::Status;
use burrow_config::{ClientEntry, Entry, ServiceEntry};
use burrow_ui_kit::{ChartData, Tone};

use crate::format;
use crate::route::Route;
use crate::screens::paths::clients_of;

pub(crate) use page::Overview;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub tone: Tone,
    pub title: String,
    pub detail: String,
    pub to: Option<Route>,
    pub field: Option<Vec<String>>,
}

fn rank(t: Tone) -> u8 {
    match t {
        Tone::Danger => 0,
        Tone::Warning => 1,
        _ => 2,
    }
}

fn names(v: &Value, section: &str) -> Vec<String> {
    v.get(section)
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn link_of(path: &[String]) -> Option<Route> {
    match path {
        [s, n, ..] if crate::route::SECTIONS.contains(&s.as_str()) => {
            Some(Route::Detail(s.clone(), n.clone()))
        }
        [_, ..] => Some(Route::Settings),
        [] => None,
    }
}

pub fn attention(
    values: &Value,
    state: Option<&State>,
    status: Option<&Status>,
    now: u64,
) -> Vec<Item> {
    let mut out = Vec::new();
    if let Some(st) = state {
        state_items(st, &mut out);
    }
    if let Some(status) = status {
        let (by_client, own) = service_items(values, status);
        for (c, (down, partial)) in by_client {
            out.push(client_item(c, down, partial, status));
        }
        out.extend(own);
        out.extend(certs::items(status, now));
    }
    out.sort_by_key(|i| rank(i.tone));
    out
}

fn state_items(st: &State, out: &mut Vec<Item>) {
    if !st.running.ok {
        out.push(Item {
            key: "running".into(),
            tone: Tone::Danger,
            title: "The config file has errors".into(),
            detail: "The server keeps running the last good version until the file is fixed."
                .into(),
            to: Some(Route::About),
            field: None,
        });
    }
    for p in &st.runtime.restart_pending {
        out.push(Item {
            key: format!("restart:{p}"),
            tone: Tone::Info,
            title: format!("Restart needed for {p}"),
            detail:
                "Applied to the file; the running server keeps the old value until it restarts."
                    .into(),
            to: Some(Route::Settings),
            field: None,
        });
    }
    for d in st.diags.iter().filter(|d| d.severity == Severity::Warning) {
        out.push(Item {
            key: format!(
                "diag:{}:{}",
                d.code,
                d.path.as_ref().map(|p| p.join(".")).unwrap_or_default()
            ),
            tone: Tone::Warning,
            title: burrow_admin_logic::wording::shown(&d.code, &d.message),
            detail: d
                .help
                .clone()
                .map(|h| format!("{} · {h}", d.code))
                .unwrap_or_else(|| d.code.clone()),
            to: d.path.as_deref().and_then(link_of),
            field: None,
        });
    }
}

fn service_items(
    values: &Value,
    status: &Status,
) -> (std::collections::BTreeMap<String, Hits>, Vec<Item>) {
    let online = |c: &str| {
        status
            .clients
            .iter()
            .find(|x| x.name == c)
            .is_some_and(|x| x.connected)
    };
    let mut by_client: std::collections::BTreeMap<String, Hits> = std::collections::BTreeMap::new();
    let mut own = Vec::new();
    if let Some(services) = ServiceEntry::entries(values) {
        for (name, svc) in services {
            let cs = clients_of(svc);
            let up: Vec<&String> = cs.iter().filter(|c| online(c)).collect();
            let off: Vec<&String> = cs.iter().filter(|c| !online(c)).collect();
            let st = status.services.iter().find(|x| &x.name == name);
            let to = Some(Route::Detail(ServiceEntry::section().into(), name.clone()));
            if up.is_empty() {
                if let [c] = off.as_slice() {
                    by_client
                        .entry((*c).clone())
                        .or_default()
                        .0
                        .push(name.clone());
                    continue;
                }
                own.push(Item {
                    key: format!("down:{name}"),
                    tone: Tone::Danger,
                    title: format!("Service {name} is down"),
                    detail: if cs.is_empty() {
                        "It has no client.".to_string()
                    } else {
                        format!(
                            "None of its {} clients is online: {}.",
                            cs.len(),
                            cs.join(", ")
                        )
                    },
                    to,
                    field: None,
                });
                continue;
            }
            let failing: Vec<&str> = st
                .map(|x| {
                    x.health
                        .iter()
                        .filter(|h| !h.up)
                        .map(|h| h.client.as_str())
                        .collect()
                })
                .unwrap_or_default();
            if !failing.is_empty() {
                own.push(Item {
                    key: format!("health:{name}"),
                    tone: Tone::Warning,
                    title: format!("Service {name}: health check failing"),
                    detail: format!("Failing on {}.", failing.join(", ")),
                    to,
                    field: None,
                });
            } else if let [c] = off.as_slice() {
                by_client.entry((*c).clone()).or_default().1.push((
                    name.clone(),
                    up.len(),
                    cs.len(),
                ));
            } else if !off.is_empty() {
                own.push(Item {
                    key: format!("partial:{name}"),
                    tone: Tone::Warning,
                    title: format!(
                        "Service {name} runs on {} of {} clients",
                        up.len(),
                        cs.len()
                    ),
                    detail: format!(
                        "Offline: {}.",
                        off.iter()
                            .map(|c| c.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    to,
                    field: None,
                });
            }
        }
    }
    (by_client, own)
}

fn client_item(
    c: String,
    down: Vec<String>,
    partial: Vec<(String, usize, usize)>,
    status: &Status,
) -> Item {
    let mut parts = Vec::new();
    if !down.is_empty() {
        parts.push(format!(
            "{} {} down",
            and_list(&down),
            if down.len() == 1 { "is" } else { "are" }
        ));
    }
    let mut shapes: Vec<(usize, usize)> = partial.iter().map(|(_, u, n)| (*u, *n)).collect();
    shapes.sort();
    shapes.dedup();
    for (u, n) in shapes {
        let names: Vec<String> = partial
            .iter()
            .filter(|(_, a, b)| (*a, *b) == (u, n))
            .map(|(x, _, _)| x.clone())
            .collect();
        parts.push(format!(
            "{} {} on {u} of {n} clients",
            and_list(&names),
            if names.len() == 1 { "runs" } else { "run" }
        ));
    }
    let when = format::left_of(
        &c,
        status
            .clients
            .iter()
            .find(|x| x.name == c)
            .and_then(|x| x.left_secs_ago),
        status.uptime_secs,
    );
    Item {
        key: format!("client:{c}"),
        tone: if down.is_empty() {
            Tone::Warning
        } else {
            Tone::Danger
        },
        title: format!("Client {c} is offline"),
        detail: format!("{} · {when}", parts.join("; ")),
        to: Some(Route::Detail(ClientEntry::section().into(), c)),
        field: None,
    }
}

type Hits = (Vec<String>, Vec<(String, usize, usize)>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Down,
    Partial,
    Health,
}

pub fn service_problems(values: &Value, status: Option<&Status>) -> Vec<(String, Problem)> {
    let Some(status) = status else {
        return Vec::new();
    };
    let online = |c: &str| {
        status
            .clients
            .iter()
            .find(|x| x.name == c)
            .is_some_and(|x| x.connected)
    };
    let mut out: Vec<(String, Problem)> = ServiceEntry::entries(values)
        .map(|m| {
            m.iter()
                .filter_map(|(name, svc)| {
                    let cs = clients_of(svc);
                    let up = cs.iter().filter(|c| online(c)).count();
                    let failing = status
                        .services
                        .iter()
                        .find(|x| &x.name == name)
                        .is_some_and(|x| x.health.iter().any(|h| !h.up));
                    let p = if up == 0 {
                        Problem::Down
                    } else if failing {
                        Problem::Health
                    } else if up < cs.len() {
                        Problem::Partial
                    } else {
                        return None;
                    };
                    Some((name.clone(), p))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn healthy_detail(p: &[(String, Problem)]) -> String {
    match p {
        [] => "all up".into(),
        [(n, Problem::Down)] => format!("{n} is down"),
        [(n, Problem::Health)] => format!("{n} fails its health check"),
        [(n, Problem::Partial)] => format!("{n} runs on fewer clients"),
        many => format!("{} with problems", many.len()),
    }
}

fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

pub fn away(values: &Value, status: Option<&Status>) -> Vec<Item> {
    let Some(status) = status else {
        return Vec::new();
    };
    names(values, ClientEntry::section())
        .into_iter()
        .filter_map(|c| {
            let st = status.clients.iter().find(|x| x.name == c);
            if st.is_some_and(|x| x.connected) {
                return None;
            }
            let serves: Vec<String> = ServiceEntry::entries(values)
                .map(|m| {
                    m.iter()
                        .filter(|(_, v)| clients_of(v).contains(&c))
                        .map(|(n, _)| n.clone())
                        .collect()
                })
                .unwrap_or_default();
            let when = format::left_of(&c, st.and_then(|x| x.left_secs_ago), status.uptime_secs);
            Some(Item {
                key: format!("away:{c}"),
                tone: Tone::Neutral,
                title: c.clone(),
                detail: format!(
                    "{} · {when}",
                    if serves.is_empty() {
                        "no services".to_string()
                    } else {
                        format!("serves {}", serves.join(", "))
                    }
                ),
                to: Some(Route::Detail(ClientEntry::section().into(), c)),
                field: None,
            })
        })
        .collect()
}

pub fn x_ticks(window: usize) -> Vec<String> {
    match window {
        60 => vec![
            "−60 s".into(),
            "−45 s".into(),
            "−30 s".into(),
            "−15 s".into(),
            "now".into(),
        ],
        300 => vec![
            "−5 min".into(),
            "−4 min".into(),
            "−3 min".into(),
            "−2 min".into(),
            "−1 min".into(),
            "now".into(),
        ],
        _ => vec![
            "−15 min".into(),
            "−10 min".into(),
            "−5 min".into(),
            "now".into(),
        ],
    }
}

fn tick(v: f64) -> String {
    if v >= 10.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else if v >= 1.0 {
        format!("{v:.1}")
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').to_string()
    }
}

pub fn chart(a: &[Option<f64>], b: &[Option<f64>], window: usize) -> ChartData {
    let take = |s: &[Option<f64>]| -> Vec<Option<f64>> {
        s[s.len().saturating_sub(window)..]
            .iter()
            .map(|v| v.map(mbit))
            .collect()
    };
    let (a, b) = (take(a), take(b));
    let top = a
        .iter()
        .chain(b.iter())
        .flatten()
        .copied()
        .fold(0.0_f64, f64::max);
    let max = if top > 0.0 { axis_max(top) } else { 20.0 };
    let samples = a.iter().filter(|v| v.is_some()).count();
    let empty_note = (a.len() < 30 && a.len() * 2 < window).then(|| {
        format!(
            "Recording since you opened this page · {} so far",
            format::span(a.len() as u64)
        )
    });
    ChartData {
        a: if samples < 2 { Vec::new() } else { a },
        b: if samples < 2 { Vec::new() } else { b },
        slots: window,
        max,
        y_ticks: (0..5).map(|i| tick(max * i as f64 / 4.0)).collect(),
        x_ticks: x_ticks(window),
        empty_note,
    }
}

pub fn old_server_item(server: &str, ui: &str) -> Option<Item> {
    burrow_admin_logic::version::server_is_older(server, ui).then(|| Item {
        key: "old-server".into(),
        tone: Tone::Warning,
        title: format!("This server runs burrow v{server}, older than this burrow admin (v{ui})"),
        detail: "Update it for the newest settings and help: run burrow update on the server as root or administrator, or pull the new image for Docker; restart a server you run by hand.".into(),
        to: Some(Route::About),
        field: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_admin_proto::serde_json::json;
    use burrow_admin_proto::status::ClientStatus;

    fn status(online: &[&str], offline: &[&str]) -> Status {
        let mut st: Status = burrow_admin_proto::serde_json::from_value(json!({
            "version": "0.1.0", "uptime_secs": 10, "listening": [], "handshakes_refused": 0, "udp_peers": 0,
            "clients": [], "services": [], "visitors": []
        }))
        .unwrap();
        for (n, on) in online
            .iter()
            .map(|n| (n, true))
            .chain(offline.iter().map(|n| (n, false)))
        {
            st.clients.push(ClientStatus {
                name: n.to_string(),
                connected: on,
                connected_secs: on.then_some(60),
                from: None,
                open: 0,
                idle_channels: 0,
                left_secs_ago: (!on).then_some(172_800),
                local: false,
                ..ClientStatus::default()
            });
        }
        st
    }

    #[test]
    fn an_older_server_needs_attention_until_it_is_updated() {
        let item = old_server_item("0.1.0", "0.1.2").expect("an older server is an item");
        assert_eq!(item.tone, Tone::Warning);
        assert_eq!(
            item.title,
            "This server runs burrow v0.1.0, older than this burrow admin (v0.1.2)"
        );
        assert_eq!(
            item.detail,
            "Update it for the newest settings and help: run burrow update on the server as root or administrator, or pull the new image for Docker; restart a server you run by hand."
        );
        assert_eq!(item.to, Some(Route::About));
        assert_eq!(old_server_item("0.1.2", "0.1.2"), None);
        assert_eq!(old_server_item("0.1.3", "0.1.2"), None);
        assert_eq!(old_server_item("garbage", "0.1.2"), None);
    }

    #[test]
    fn needs_attention_groups_by_the_offline_client_and_away_lists_the_rest() {
        let v = json!({
            "clients": {"a": {}, "b": {}, "c": {}, "d": {}},
            "services": {
                "aa-partial": {"clients": ["a", "b"], "target": "x:1"},
                "zz-down": {"client": "b", "target": "x:2"},
                "ok": {"client": "a", "target": "x:3"},
                "both": {"clients": ["c", "d"], "target": "x:4"}
            }
        });
        let st = status(&["a"], &["b", "c", "d"]);
        let items = attention(&v, None, Some(&st), 0);
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles, ["Client b is offline", "Service both is down"]);
        assert_eq!(
            items[0].detail,
            "zz-down is down; aa-partial runs on 1 of 2 clients · left 2d 0h ago"
        );
        assert_eq!(items[0].tone, Tone::Danger);
        assert_eq!(items[1].detail, "None of its 2 clients is online: c, d.");
        let a = away(&v, Some(&st));
        assert_eq!(
            a.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["b", "c", "d"],
            "every offline client is listed in Clients away"
        );
        assert!(a[1].detail.starts_with("serves both · "), "{}", a[1].detail);
        assert!(attention(&v, None, None, 0).is_empty());
    }

    #[test]
    fn services_healthy_counts_services_whose_client_is_offline() {
        let v = json!({
            "clients": {"a": {}, "b": {}},
            "services": {
                "web": {"client": "a", "target": "x:1"},
                "backup": {"client": "b", "target": "x:2"},
                "both": {"clients": ["a", "b"], "target": "x:3"}
            }
        });
        let st = status(&["a"], &["b"]);
        let p = service_problems(&v, Some(&st));
        assert_eq!(
            p,
            [
                ("backup".to_string(), Problem::Down),
                ("both".to_string(), Problem::Partial)
            ]
        );
        assert_eq!(healthy_detail(&p), "2 with problems");
        assert_eq!(healthy_detail(&p[..1]), "backup is down");
        assert_eq!(healthy_detail(&p[1..]), "both runs on fewer clients");
        assert_eq!(healthy_detail(&[]), "all up");
    }

    #[test]
    fn the_chart_scales_to_mbit_and_keeps_gaps() {
        let mb = 1_000_000.0 / 8.0;
        let a = vec![Some(10.0 * mb), None, Some(50.0 * mb)];
        let b = vec![Some(1.0 * mb), None, Some(2.0 * mb)];
        let d = chart(&a, &b, 60);
        assert_eq!(d.max, 60.0);
        assert_eq!(d.a[1], None);
        assert!((d.a[2].unwrap() - 50.0).abs() < 1e-9);
        assert_eq!(d.y_ticks, ["0", "15", "30", "45", "60"]);
        assert_eq!(d.x_ticks.last().map(String::as_str), Some("now"));
        assert!(d.empty_note.unwrap().contains("3s so far"));
        assert!(chart(&a[..1], &b[..1], 900).a.is_empty());
        let full: Vec<Option<f64>> = vec![Some(mb); 1000];
        let d = chart(&full, &full, 900);
        assert_eq!(d.a.len(), 900);
        assert!(d.empty_note.is_none());
    }
}
