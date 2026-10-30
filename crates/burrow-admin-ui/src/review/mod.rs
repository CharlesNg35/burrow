mod detail;
mod guard;
mod page;

use burrow_admin_proto::plan::{Change, ChangeKind, Diff, Level, Plan};
use burrow_admin_proto::serde_json::Value;
use burrow_config::{Entry, ServiceEntry};
use burrow_ui_kit::{EdgeState, HopState, PathData, Tone};

use crate::screens::paths::{self, Problems};

pub(crate) use page::{ConflictDialog, ReviewPage, recheck, stale};

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub head: Change,
    pub parts: Vec<Change>,
}

pub fn change_groups(plan: &Plan) -> Vec<Group> {
    let mut out: Vec<Group> = plan
        .changes
        .iter()
        .filter(|c| {
            c.part_of
                .as_ref()
                .is_none_or(|g| !plan.changes.iter().any(|h| h.path == *g))
        })
        .map(|c| Group {
            head: c.clone(),
            parts: Vec::new(),
        })
        .collect();
    for c in &plan.changes {
        if let Some(g) = c
            .part_of
            .as_ref()
            .and_then(|g| out.iter_mut().find(|x| x.head.path == *g))
        {
            g.parts.push(c.clone());
        }
    }
    let entity = |g: &Group| {
        g.head
            .path
            .first()
            .is_some_and(|s| crate::route::SECTIONS.contains(&s.as_str()))
    };
    let settings: Vec<Group> = out.iter().filter(|g| !entity(g)).cloned().collect();
    if settings.len() > 1 {
        out.retain(entity);
        let names: Vec<String> = settings.iter().map(|g| g.head.path.join(".")).collect();
        out.push(Group {
            head: Change {
                kind: ChangeKind::Set,
                path: vec!["settings".into()],
                summary: format!("Server settings: {}", names.join(", ")),
                part_of: None,
            },
            parts: settings.into_iter().map(|g| g.head).collect(),
        });
    }
    out
}

pub const REVIEW_ON_NEW: &str = "Review my changes on the new version";

fn section_label(section: &str, n: usize) -> String {
    let one = match burrow_config::entity_of_section(section) {
        Some(burrow_admin_proto::schema::Entity::Client) => burrow_config::ClientEntry::ENTITY,
        Some(burrow_admin_proto::schema::Entity::Visitor) => burrow_config::VisitorEntry::ENTITY,
        _ => burrow_config::ServiceEntry::ENTITY,
    };
    if n == 1 {
        one.to_string()
    } else {
        format!("{one}s")
    }
}

pub fn place_of(root: &burrow_admin_proto::schema::Node, path: &[String]) -> String {
    let label = |p: &[String]| {
        root.at(p)
            .map(|n| n.label.clone())
            .filter(|l| !l.is_empty())
    };
    match path {
        [sec, name, rest @ ..] if crate::route::SECTIONS.contains(&sec.as_str()) => {
            let head = format!("{} {name}", section_label(sec, 1));
            let mut head = head[..1].to_uppercase() + &head[1..];
            if let Some(k) = rest.first()
                && let Some(l) = label(&[sec.clone(), "*".into(), k.clone()])
            {
                head.push_str(&format!(" · {l}"));
            }
            head
        }
        [k, rest @ ..] => match label(std::slice::from_ref(k)) {
            Some(top) => match rest.first().and_then(|r| label(&[k.clone(), r.clone()])) {
                Some(l) => format!("{top} · {l}"),
                None => top,
            },
            None => path.join("."),
        },
        [] => String::new(),
    }
}

fn plain_place(path: &[String]) -> String {
    match path {
        [sec, name, ..] if crate::route::SECTIONS.contains(&sec.as_str()) => {
            let one = section_label(sec, 1);
            format!("{}{} · {name}", one[..1].to_uppercase(), &one[1..])
        }
        _ => path.join(" · "),
    }
}

pub fn group_line(g: &Group) -> String {
    if g.parts.is_empty() {
        return plain_place(&g.head.path);
    }
    if g.head
        .path
        .first()
        .is_none_or(|s| !crate::route::SECTIONS.contains(&s.as_str()))
    {
        return g
            .parts
            .iter()
            .map(|p| p.summary.clone())
            .collect::<Vec<_>>()
            .join("; ");
    }
    let verb = match g.head.kind {
        ChangeKind::Add => "added to",
        ChangeKind::Remove => "removed from",
        _ => "updated in",
    };
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for p in &g.parts {
        let (Some(sec), Some(name)) = (p.path.first(), p.path.get(1)) else {
            continue;
        };
        match sections.iter_mut().find(|(s, _)| s == sec) {
            Some((_, names)) => names.push(name.clone()),
            None => sections.push((sec.clone(), vec![name.clone()])),
        }
    }
    let text = sections
        .iter()
        .map(|(sec, names)| format!("{} {}", section_label(sec, names.len()), names.join(", ")))
        .collect::<Vec<_>>()
        .join(" and ");
    format!("{verb} {text}")
}

pub fn nothing_to_apply(plan: &Plan) -> bool {
    plan.changes.is_empty() && plan.diff.hunks.is_empty()
}

fn diff_counts(d: &Diff) -> (usize, usize) {
    let lines = d.hunks.iter().flat_map(|h| h.lines.iter());
    lines.fold((0, 0), |(a, r), (k, _)| match k.as_str() {
        "+" => (a + 1, r),
        "-" => (a, r + 1),
        _ => (a, r),
    })
}

fn kind_look(k: ChangeKind) -> (&'static str, Tone) {
    match k {
        ChangeKind::Add => ("plus", Tone::Success),
        ChangeKind::Remove => ("trash", Tone::Danger),
        ChangeKind::Rename => ("arrow", Tone::Accent),
        ChangeKind::Set | ChangeKind::Other => ("edit", Tone::Accent),
    }
}

fn impact_icon(l: Level) -> (&'static str, Tone) {
    match l {
        Level::Danger => ("alert-circle", Tone::Danger),
        Level::Warning => ("alert", Tone::Warning),
        Level::Info => ("info", Tone::Info),
    }
}

fn service_paths(
    base: &Value,
    draft: &Value,
    status: Option<&burrow_admin_proto::status::Status>,
    reserved: &[String],
) -> Vec<(String, PathData)> {
    let empty = serde_json_map();
    let b = ServiceEntry::entries(base).unwrap_or(&empty);
    let d = ServiceEntry::entries(draft).unwrap_or(&empty);
    let none = Problems {
        source: None,
        clients: None,
        ports: None,
    };
    let mut names: Vec<&String> = b.keys().chain(d.keys()).collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter_map(|n| {
            let (old, new) = (b.get(n), d.get(n));
            if old == new {
                return None;
            }
            let mark = |p: &mut PathData, hop: HopState, edge: EdgeState| {
                for h in &mut p.hops {
                    h.state = hop;
                    h.dot = None;
                }
                for e in &mut p.edges {
                    e.state = edge;
                }
            };
            let data = match (old, new) {
                (Some(o), None) => {
                    let mut p = paths::service(n, o, status, &none, true, reserved);
                    mark(&mut p, HopState::Removed, EdgeState::Removed);
                    p.summary = format!("Service {n} is removed");
                    p
                }
                (None, Some(x)) => {
                    let mut p = paths::service(n, x, status, &none, true, reserved);
                    mark(&mut p, HopState::Added, EdgeState::Added);
                    p.summary = format!("Service {n} is added");
                    p
                }
                (Some(o), Some(x)) => {
                    let before = paths::service(n, o, status, &none, true, reserved);
                    let mut p = paths::service(n, x, status, &none, true, reserved);
                    for (h, was) in p.hops.iter_mut().zip(before.hops.iter()) {
                        h.dot = None;
                        h.state = if h.value != was.value || h.unit != was.unit {
                            HopState::Added
                        } else {
                            HopState::Normal
                        };
                        if h.value != was.value && h.icon == was.icon {
                            h.badge = Some((format!("was {}", was.value), Tone::Neutral));
                        }
                    }
                    for e in &mut p.edges {
                        e.state = EdgeState::Live;
                    }
                    p.summary = format!("Service {n} changes");
                    p
                }
                (None, None) => return None,
            };
            Some((n.clone(), data))
        })
        .collect()
}

fn serde_json_map() -> burrow_admin_proto::serde_json::Map<String, Value> {
    burrow_admin_proto::serde_json::Map::new()
}

fn session_ip(from: &str) -> String {
    let host = match from.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.is_empty() => h,
        _ => from,
    };
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diagnostic_says_where_in_the_words_of_the_form() {
        let root =
            <burrow_config::ServerConfig as burrow_config::admin_schema::AdminSchema>::node();
        let p = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            place_of(&root, &p(&["visitors", "laptop", "max_connections"])),
            "Visitor laptop · Maximum connections"
        );
        assert_eq!(
            place_of(&root, &p(&["services", "web", "allow_from", "1"])),
            "Service web · Allow from"
        );
        assert_eq!(
            place_of(&root, &p(&["pool", "min_idle"])),
            "Pool of idle channels · Idle channels kept ready"
        );
        assert_eq!(place_of(&root, &p(&["nonsense"])), "nonsense");
    }
    use burrow_admin_proto::plan::Hunk;
    use burrow_admin_proto::serde_json::json;

    #[test]
    fn the_review_marks_removed_added_and_changed_services() {
        let base = json!({"services": {"ssh": {"client": "a", "bind": "0.0.0.0:2222", "target": "x:22"}, "web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"}}});
        let draft = json!({"services": {"web": {"client": "a", "bind": "0.0.0.0:8443", "target": "x:80"}, "db": {"client": "a", "target": "y:5432"}}});
        let p = service_paths(&base, &draft, None, &[]);
        let names: Vec<&str> = p.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["db", "ssh", "web"]);
        assert!(p[0].1.hops.iter().all(|h| h.state == HopState::Added));
        assert!(p[1].1.hops.iter().all(|h| h.state == HopState::Removed));
        let web = &p[2].1;
        assert_eq!(web.hops[1].state, HopState::Added);
        assert_eq!(web.hops[1].badge.as_ref().unwrap().0, "was 0.0.0.0:443");
        assert_eq!(web.hops[3].state, HopState::Normal);
        let joined = json!({"services": {"web": {"clients": ["a", "b"], "bind": "0.0.0.0:443", "target": "x:80"}}});
        let base =
            json!({"services": {"web": {"client": "a", "bind": "0.0.0.0:443", "target": "x:80"}}});
        let p = service_paths(&base, &joined, None, &[]);
        let clients = p[0].1.hops.iter().find(|h| h.icon == "monitor").unwrap();
        assert_eq!(
            clients.badge.as_ref().map(|b| b.0.as_str()),
            Some("was a"),
            "a hop that becomes plural still says what it was"
        );
    }

    #[test]
    fn a_new_client_and_the_services_it_joins_are_one_group() {
        let ch = |kind, path: &[&str], summary: &str, part_of: Option<&[&str]>| Change {
            kind,
            path: path.iter().map(|x| x.to_string()).collect(),
            summary: summary.into(),
            part_of: part_of.map(|g| g.iter().map(|x| x.to_string()).collect()),
        };
        let office: &[&str] = &["clients", "office"];
        let plan = Plan {
            changes: vec![
                ch(
                    ChangeKind::Add,
                    office,
                    "New client office (1 secret)",
                    None,
                ),
                ch(
                    ChangeKind::Set,
                    &["services", "web"],
                    "Service web: clients",
                    Some(office),
                ),
                ch(
                    ChangeKind::Set,
                    &["services", "db"],
                    "Service db: clients",
                    Some(office),
                ),
                ch(
                    ChangeKind::Set,
                    &["visitors", "lap"],
                    "Visitor lap: reach",
                    Some(office),
                ),
                ch(
                    ChangeKind::Set,
                    &["services", "ssh"],
                    "Service ssh: target",
                    None,
                ),
                ch(
                    ChangeKind::Set,
                    &["services", "x"],
                    "Service x: clients",
                    Some(&["clients", "gone"]),
                ),
            ],
            ..Plan::default()
        };
        let g = change_groups(&plan);
        assert_eq!(g.len(), 3, "{g:?}");
        assert_eq!(g[0].parts.len(), 3);
        assert_eq!(
            group_line(&g[0]),
            "added to services web, db and visitor lap"
        );
        assert_eq!(group_line(&g[1]), "Service · ssh");
        assert_eq!(
            g[2].head.path,
            ["services", "x"],
            "a group whose head is missing stands alone"
        );
        assert!(!nothing_to_apply(&plan));
        assert!(nothing_to_apply(&Plan::default()));
        let settings = Plan {
            changes: vec![
                ch(ChangeKind::Set, &["pool"], "pool: min_idle 4 → 8", None),
                ch(
                    ChangeKind::Set,
                    &["limits"],
                    "limits: max_pending 512 → 9",
                    None,
                ),
            ],
            ..Plan::default()
        };
        let g = change_groups(&settings);
        assert_eq!(g.len(), 1, "server settings are one change");
        assert_eq!(g[0].head.summary, "Server settings: pool, limits");
    }

    #[test]
    fn diff_counts_and_session_address() {
        let d = Diff {
            unified: String::new(),
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 2,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    ("+".into(), "a".into()),
                    ("-".into(), "b".into()),
                    ("-".into(), "c".into()),
                    (" ".into(), "d".into()),
                ],
            }],
        };
        assert_eq!(diff_counts(&d), (1, 2));
        assert_eq!(session_ip("203.0.113.7:51000"), "203.0.113.7");
        assert_eq!(session_ip("[2001:db8::1]:443"), "2001:db8::1");
        assert_eq!(session_ip("203.0.113.7"), "203.0.113.7");
    }
}
