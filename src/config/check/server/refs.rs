use crate::base::suggest::{closest, quoted_list};
use crate::config::check::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::tree::{Node, Span, Value};
use crate::policy::allowlist::AllowList;
use burrow_config::SELF;

fn declared<'a>(root: &'a Node, section: &str) -> Vec<&'a str> {
    match root.get(section).map(|c| &c.value) {
        Some(Value::Map(entries)) => entries.iter().map(|e| e.key.as_str()).collect(),
        _ => Vec::new(),
    }
}

fn unknown_ref(kind: &str, name: &str, known: &[&str], by: String, span: Span) -> Diagnostic {
    let section = format!("{kind}s");
    let defined = if known.is_empty() {
        format!("no {section} are defined")
    } else {
        format!("defined {section}: {}", quoted_list(known.iter().copied()))
    };
    let help = match closest(name, known.iter().copied()) {
        Some(s) => format!("did you mean \"{s}\"? {defined}"),
        None => format!("add it under \"{section}:\"; {defined}"),
    };
    let code = if kind == "client" {
        "unknown-client"
    } else {
        "unknown-visitor"
    };
    Diagnostic::error(
        code,
        format!("unknown {kind} \"{name}\" referenced by {by}"),
    )
    .at(span)
    .label(format!("no {kind} with this name"))
    .help(help)
}

pub fn check_refs(root: &Node, out: &mut Vec<Diagnostic>) {
    let clients = declared(root, "clients");
    let visitors = declared(root, "visitors");
    if let Some(Value::Map(services)) = root.get("services").map(|s| &s.value) {
        for svc in services {
            let by = || format!("service \"{}\"", svc.key);
            if let Some(client) = svc.value.get("client")
                && let Some(name) = client.as_str()
                && name != SELF
                && !clients.contains(&name)
            {
                out.push(unknown_ref("client", name, &clients, by(), client.span));
            }
            if let Some(Value::Seq(items)) = svc.value.get("clients").map(|v| &v.value) {
                for item in items {
                    if let Some(name) = item.as_str()
                        && name != SELF
                        && !clients.contains(&name)
                    {
                        out.push(unknown_ref("client", name, &clients, by(), item.span));
                    }
                }
            }
            if let Some(Value::Seq(items)) = svc.value.get("visitors").map(|v| &v.value) {
                for item in items {
                    if let Some(name) = item.as_str()
                        && !visitors.contains(&name)
                    {
                        out.push(unknown_ref("visitor", name, &visitors, by(), item.span));
                    }
                }
            }
        }
    }
    if let Some(Value::Map(entries)) = root.get("visitors").map(|v| &v.value) {
        for visitor in entries {
            let Some(Value::Map(reach)) = visitor.value.get("reach").map(|r| &r.value) else {
                continue;
            };
            for r in reach {
                if r.key != SELF && !clients.contains(&r.key.as_str()) {
                    let by = format!("\"reach\" of visitor \"{}\"", visitor.key);
                    out.push(unknown_ref("client", &r.key, &clients, by, r.key_span));
                }
            }
        }
    }
}

pub(super) fn check_reach(
    visitor: &str,
    client: &str,
    given: &burrow_config::Reach,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<AllowList> {
    let entries = given.entries();
    let many = matches!(given, burrow_config::Reach::Many(_));
    let at = |i: usize| {
        let path = ["visitors", visitor, "reach", client];
        if many {
            spans.value(&[&path[..], &[i.to_string().as_str()]].concat())
        } else {
            spans.value(&path)
        }
    };
    if let [one] = entries
        && !many
        && one.trim() == "any"
    {
        return Some(AllowList::All);
    }
    if let [one] = entries
        && !many
        && one.chars().all(|c| c.is_ascii_alphabetic())
    {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!("invalid reach \"{one}\" for client \"{client}\""),
            )
            .at(at(0))
            .label("expected any or IP ranges")
            .help(format!(
                "write {client}: any for any target its allow list permits, or narrow it with ranges, for example {client}: [192.168.1.0/24]"
            )),
        );
        return None;
    }
    if entries.is_empty() {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!("reach for client \"{client}\" of visitor \"{visitor}\" is an empty list, which lets it reach nothing"),
            )
            .at(spans.value(&["visitors", visitor, "reach", client]))
            .help(format!("remove \"{client}\" from reach, or list what it may reach, for example {client}: [192.168.1.0/24]")),
        );
        return None;
    }
    if entries.len() > crate::wire::protocol::MAX_ALLOW_RULES {
        out.push(
            Diagnostic::error(
                "invalid-reach",
                format!(
                    "reach for client \"{client}\" of visitor \"{visitor}\" has {} rules, the limit is {}",
                    entries.len(),
                    crate::wire::protocol::MAX_ALLOW_RULES
                ),
            )
            .at(spans.value(&["visitors", visitor, "reach", client]))
            .help("combine them into wider ranges, for example 10.0.0.0/8 instead of each /24"),
        );
        return None;
    }
    let mut rules = Vec::new();
    let mut ok = true;
    for (i, entry) in entries.iter().enumerate() {
        if entry.trim() == "any" {
            out.push(
                Diagnostic::error(
                    "invalid-reach",
                    "\"any\" cannot be combined with other rules",
                )
                .at(at(i))
                .help(format!("write {client}: any alone, or list only rules")),
            );
            ok = false;
            continue;
        }
        match crate::config::check::client::parse_rule(entry, "reach", at(i), out) {
            Some(r) => rules.push(r),
            None => ok = false,
        }
    }
    ok.then_some(AllowList::Rules(rules))
}
