use burrow_admin_proto::text_search::TextMatch;
use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Recent,
    Problems,
    File,
    Service,
    Client,
    Visitor,
    Text,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Recent => "Recent files",
            Kind::Problems => "Files with problems",
            Kind::File => "Files",
            Kind::Service => "Services",
            Kind::Client => "Clients",
            Kind::Visitor => "Visitors",
            Kind::Text => "In the text",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Recent | Kind::File => "file",
            Kind::Problems => "warning",
            Kind::Service => crate::entity::of_section(ServiceEntry::section()).icon,
            Kind::Client => crate::entity::of_section(ClientEntry::section()).icon,
            Kind::Visitor => crate::entity::of_section(VisitorEntry::section()).icon,
            Kind::Text => "search",
        }
    }

    fn of_section(section: &str) -> Option<Kind> {
        [
            (ServiceEntry::section(), Kind::Service),
            (ClientEntry::section(), Kind::Client),
            (VisitorEntry::section(), Kind::Visitor),
        ]
        .into_iter()
        .find(|(s, _)| *s == section)
        .map(|(_, k)| k)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub label: String,
    pub file: String,
    pub path: Option<Vec<String>>,
    pub line: Option<(u32, u32)>,
    pub note: String,
}

impl Found {
    fn file(name: &str, note: &str) -> Found {
        Found {
            label: name.to_string(),
            file: name.to_string(),
            path: None,
            line: None,
            note: note.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub path: Vec<String>,
    pub file: String,
}

pub struct Sources<'a> {
    pub files: &'a [String],
    pub items: &'a [Item],
    pub recent: &'a [String],
    pub problems: &'a [String],
    pub text: &'a [TextMatch],
}

fn subsequence(q: &str, c: &str) -> bool {
    let mut rest = c.chars();
    q.chars().all(|x| rest.any(|y| y == x))
}

pub fn rank(query: &str, candidate: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    let c = candidate.to_lowercase();
    if q.is_empty() {
        return Some(1);
    }
    if c == q {
        Some(0)
    } else if c.starts_with(&q) {
        Some(1)
    } else if c.contains(&q) {
        Some(2)
    } else if subsequence(&q, &c) {
        Some(3)
    } else {
        None
    }
}

pub fn file_rank(query: &str, file: &str) -> Option<u8> {
    let base = file.rsplit('/').next().unwrap_or(file);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    [stem, base, file]
        .iter()
        .filter_map(|c| rank(query, c))
        .min()
}

fn sorted(mut hits: Vec<(u8, Found)>) -> Vec<Found> {
    hits.sort_by(|a, b| (a.0, &a.1.label).cmp(&(b.0, &b.1.label)));
    hits.into_iter().map(|(_, f)| f).collect()
}

fn problem_note(s: &Sources, file: &str) -> &'static str {
    if s.problems.iter().any(|p| p == file) {
        "has problems"
    } else {
        ""
    }
}

fn starting(s: &Sources) -> Vec<(Kind, Vec<Found>)> {
    let main = s.files.first().map(String::as_str).unwrap_or_default();
    let mut recent: Vec<&str> = s
        .recent
        .iter()
        .map(String::as_str)
        .filter(|r| s.files.iter().any(|f| f == r))
        .collect();
    if !main.is_empty() && !recent.contains(&main) {
        recent.push(main);
    }
    let mut out = Vec::new();
    if !s.problems.is_empty() {
        let hits = s.problems.iter().map(|p| Found::file(p, "")).collect();
        out.push((Kind::Problems, hits));
    }
    let hits = recent
        .iter()
        .map(|r| Found::file(r, problem_note(s, r)))
        .collect();
    out.push((Kind::Recent, hits));
    out
}

pub fn find(query: &str, s: &Sources) -> Vec<(Kind, Vec<Found>)> {
    if query.trim().is_empty() {
        return starting(s);
    }
    let files = s
        .files
        .iter()
        .filter_map(|f| Some((file_rank(query, f)?, Found::file(f, problem_note(s, f)))))
        .collect();
    let mut out = vec![(Kind::File, sorted(files))];
    for kind in [Kind::Service, Kind::Client, Kind::Visitor] {
        let hits = s
            .items
            .iter()
            .filter(|i| i.path.first().and_then(|x| Kind::of_section(x)) == Some(kind))
            .filter_map(|i| {
                let name = i.path.get(1)?;
                Some((
                    rank(query, name)?,
                    Found {
                        label: name.clone(),
                        file: i.file.clone(),
                        path: Some(i.path.clone()),
                        line: None,
                        note: i.file.clone(),
                    },
                ))
            })
            .collect();
        out.push((kind, sorted(hits)));
    }
    out.push((Kind::Text, s.text.iter().map(text_hit).collect()));
    out.retain(|(_, hits)| !hits.is_empty());
    out
}

fn text_hit(m: &TextMatch) -> Found {
    Found {
        label: m.excerpt.clone(),
        file: m.file.clone(),
        path: None,
        line: Some((m.line, m.col)),
        note: format!("{}:{}", m.file, m.line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn item(section: &str, name: &str, file: &str) -> Item {
        Item {
            path: names(&[section, name]),
            file: file.into(),
        }
    }

    fn labels(groups: &[(Kind, Vec<Found>)], kind: Kind) -> Vec<String> {
        groups
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, h)| h.iter().map(|f| f.label.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_query_matches_exact_then_prefix_then_inside_then_scattered() {
        assert_eq!(rank("Web", "web"), Some(0));
        assert_eq!(rank("we", "web-17"), Some(1));
        assert_eq!(rank("17", "web-17"), Some(2));
        assert_eq!(rank("w17", "web-17"), Some(3));
        assert_eq!(rank("x", "web-17"), None);
        assert_eq!(file_rank("w17", "server.d/web-17.yaml"), Some(3));
        assert_eq!(file_rank("web-17", "server.d/web-17.yaml"), Some(0));
        assert_eq!(file_rank("server.d/w", "server.d/web-17.yaml"), Some(1));
    }

    #[test]
    fn results_come_in_groups_with_the_best_match_first() {
        let files = names(&["server.yaml", "server.d/web-17.yaml", "server.d/web.yaml"]);
        let items = vec![
            item("services", "webmail", "server.d/web.yaml"),
            item("services", "web", "server.yaml"),
            item("clients", "web-box", "server.d/web-17.yaml"),
            item("visitors", "laptop", "server.yaml"),
        ];
        let s = Sources {
            files: &files,
            items: &items,
            recent: &[],
            problems: &[],
            text: &[],
        };
        let got = find("web", &s);
        assert_eq!(
            got.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            [Kind::File, Kind::Service, Kind::Client],
            "a group without hits is left out"
        );
        assert_eq!(
            labels(&got, Kind::File),
            ["server.d/web.yaml", "server.d/web-17.yaml"]
        );
        assert_eq!(labels(&got, Kind::Service), ["web", "webmail"]);
        let svc = &got[1].1[0];
        assert_eq!(svc.file, "server.yaml");
        assert_eq!(svc.path, Some(names(&["services", "web"])));
        assert_eq!(
            labels(&find("w17", &s), Kind::File),
            ["server.d/web-17.yaml"]
        );
    }

    #[test]
    fn an_empty_query_shows_recent_files_the_main_file_and_files_with_problems() {
        let files = names(&["server.yaml", "server.d/a.yaml", "server.d/b.yaml"]);
        let recent = names(&["server.d/b.yaml", "server.d/gone.yaml"]);
        let problems = names(&["server.d/a.yaml"]);
        let s = Sources {
            files: &files,
            items: &[],
            recent: &recent,
            problems: &problems,
            text: &[],
        };
        let got = find("  ", &s);
        assert_eq!(
            labels(&got, Kind::Recent),
            ["server.d/b.yaml", "server.yaml"]
        );
        assert_eq!(labels(&got, Kind::Problems), ["server.d/a.yaml"]);
        assert_eq!(got[0].0, Kind::Problems);
        let found = find("a", &s);
        assert_eq!(found[0].1[0].note, "has problems");
    }

    #[test]
    fn text_matches_come_last_and_open_at_their_line() {
        let (text, _) = burrow_admin_proto::text_search::find(
            &[("server.d/a.yaml", "services:\n  a:\n    target: 3005\n")],
            "3005",
            50,
        );
        let s = Sources {
            files: &[],
            items: &[],
            recent: &[],
            problems: &[],
            text: &text,
        };
        let got = find("3005", &s);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, Kind::Text);
        let hit = &got[0].1[0];
        assert_eq!(
            (hit.file.as_str(), hit.line),
            ("server.d/a.yaml", Some((3, 13)))
        );
        assert_eq!(hit.label, "target: 3005");
        assert_eq!(hit.note, "server.d/a.yaml:3");
    }
}
