use std::collections::{HashMap, HashSet};

use super::layout::{Act, Line, Node, Svc, problem_svc, row_y};

pub(super) const BUNDLE_AT: usize = 200;

pub(super) struct Group {
    pub(super) client: String,
    count: usize,
    down: usize,
}

pub(super) enum Kind {
    One,
    Bundle(Group),
    Header(Group),
}

pub(super) struct Row {
    pub(super) svc: Svc,
    pub(super) kind: Kind,
}

enum Slot {
    One(Svc),
    Group(String, Vec<Svc>),
}

fn owner(s: &Svc) -> &str {
    s.clients.first().map_or("", String::as_str)
}

fn one(svc: Svc) -> Row {
    Row {
        svc,
        kind: Kind::One,
    }
}

pub(super) fn plan(svcs: Vec<Svc>, open: &HashSet<String>) -> Vec<Row> {
    if svcs.len() <= BUNDLE_AT {
        return svcs.into_iter().map(one).collect();
    }
    let mut down: HashMap<String, usize> = HashMap::new();
    for s in svcs.iter().filter(|s| problem_svc(s)) {
        *down.entry(owner(s).to_string()).or_default() += 1;
    }
    let mut slots: Vec<Slot> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for s in svcs {
        if problem_svc(&s) {
            slots.push(Slot::One(s));
            continue;
        }
        let c = owner(&s).to_string();
        match at.get(&c) {
            Some(&i) => {
                if let Slot::Group(_, members) = &mut slots[i] {
                    members.push(s);
                }
            }
            None => {
                at.insert(c.clone(), slots.len());
                slots.push(Slot::Group(c, vec![s]));
            }
        }
    }
    let mut rows = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Slot::One(svc) => rows.push(one(svc)),
            Slot::Group(client, members) => {
                let g = Group {
                    down: down.get(&client).copied().unwrap_or(0),
                    count: members.len(),
                    client,
                };
                let svc = summary(&g, &members);
                if open.contains(&g.client) {
                    rows.push(Row {
                        svc,
                        kind: Kind::Header(g),
                    });
                    rows.extend(members.into_iter().map(one));
                } else {
                    rows.push(Row {
                        svc,
                        kind: Kind::Bundle(g),
                    });
                }
            }
        }
    }
    rows
}

pub(super) fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn label(client: &str) -> String {
    if client.is_empty() {
        "no client".into()
    } else {
        client.to_string()
    }
}

fn summary(g: &Group, members: &[Svc]) -> Svc {
    Svc {
        name: label(&g.client),
        bind: members.iter().any(|m| m.bind.is_some()).then(String::new),
        web: Vec::new(),
        target: format!("{} targets", thousands(g.count)),
        protocol: String::new(),
        clients: std::iter::once(g.client.clone())
            .filter(|c| !c.is_empty())
            .collect(),
        open: members.iter().map(|m| m.open).sum(),
        rate: members.iter().map(|m| m.rate).sum(),
        health: members.first().map(|m| m.health).unwrap_or_default(),
        badge: None,
        reached: String::new(),
    }
}

fn line(g: &Group) -> String {
    let n = thousands(g.count);
    if g.down > 0 {
        format!("{n} services · {} down", thousands(g.down))
    } else {
        format!("{n} services")
    }
}

fn fold(g: &Group, open: bool) -> Option<Act> {
    Some(Act::Fold {
        client: g.client.clone(),
        open,
    })
}

pub(super) fn dress(g: &Group, port: &mut Node, target: &mut Node, lines: &mut [Line]) {
    let tip = format!(
        "Show the {} services of {} one by one",
        thousands(g.count),
        label(&g.client)
    );
    port.key = format!("bundle:{}", g.client);
    port.sub = line(g);
    port.right = "▸".into();
    port.act = fold(g, false);
    port.tip.clone_from(&tip);
    target.key = format!("targets:{}", g.client);
    target.act = fold(g, false);
    target.tip = tip;
    for l in lines {
        l.key = format!("bundle:{}/{}", g.client, l.key);
    }
}

pub(super) fn header(i: usize, g: &Group) -> Node {
    Node {
        key: format!("bundle:{}", g.client),
        col: 1,
        y: row_y(i),
        h: 42.0,
        title: label(&g.client),
        right: "▾".into(),
        sub: line(g),
        act: fold(g, true),
        tip: format!("Fold the services of {} into one row", label(&g.client)),
        ..Node::default()
    }
}

#[cfg(test)]
mod tests {
    use super::thousands;

    #[test]
    fn counts_read_with_thousands_separators() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(1240), "1,240");
        assert_eq!(thousands(5_000_000), "5,000,000");
    }
}
