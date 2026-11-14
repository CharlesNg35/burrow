use super::mask::{self, Ids, Masked, Table};
use super::unmask::unmask;
use crate::config::Format;
use crate::wire::secret::Secret;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::text_edit::placeholder;

fn root() -> Node {
    crate::config::admin_schema::server_schema().root
}

fn named(name: &str, json: bool) -> mask::Named<'_> {
    mask::Named {
        name,
        format: if json { Format::Json } else { Format::Yaml },
    }
}

fn masked(text: &str, json: bool, ids: &Ids) -> Masked {
    mask::mask(named("server.yaml", json), text, &root(), ids).unwrap()
}

fn table(m: &Masked) -> Table {
    m.kept.iter().cloned().rev().collect()
}

fn back(sent: &str, from: &Masked) -> Result<String, Vec<burrow_admin_proto::diag::Diag>> {
    unmask(named("server.yaml", false), sent, &table(from)).map(|(t, _)| t)
}

struct Styles {
    text: String,
    hidden: Vec<String>,
    fps: Vec<String>,
}

fn styles() -> Styles {
    let s: Vec<Secret> = (0..7).map(|_| Secret::generate()).collect();
    let e: Vec<String> = s.iter().map(Secret::encode).collect();
    let text = format!(
        "# the old key was {}\nlisten: 7835 # tcp\nclients:\n  home:\n    secret: {} # rotated\n  quoted:\n    secret: \"{}\"\n  single:\n    secret: '{}'\n  listed:\n    secret:\n      - {}\n      - \"${{SPARE}}\"\n  flow: {{ secret: {}, max_connections: 3 }}\n  weak:\n    secret: hunter2\n  block:\n    secret: |\n      {}\nvisitors:\n  ops:\n    secret: file:ops.secret\n    admin: write\nservices:\n  web: {{ client: home, target: 3000 }}\n",
        e[0], e[1], e[2], e[3], e[4], e[5], e[6]
    );
    let mut hidden = e.clone();
    hidden.push("hunter2".into());
    hidden.push("file:ops.secret".into());
    Styles {
        text,
        hidden,
        fps: s.iter().map(Secret::fingerprint).collect(),
    }
}

#[test]
fn every_way_of_writing_a_secret_is_masked_and_comes_back_byte_for_byte() {
    let ids = Ids::default();
    let st = styles();
    for text in [
        st.text.clone(),
        st.text.replace('\n', "\r\n"),
        format!("\u{feff}{}", st.text),
    ] {
        let m = masked(&text, false, &ids);
        for h in &st.hidden {
            assert!(!m.text.contains(h.as_str()), "{h} in\n{}", m.text);
        }
        let fp = |i: usize| placeholder(&st.fps[i]);
        for want in [
            format!("# the old key was {}", fp(0)),
            format!("    secret: {} # rotated", fp(1)),
            format!("    secret: \"{}\"", fp(2)),
            format!("    secret: '{}'", fp(3)),
            format!("      - {}", fp(4)),
            format!("  flow: {{ secret: {}, max_connections: 3 }}", fp(5)),
            format!("    secret: {}", fp(6)),
            "      - \"${SPARE}\"".to_string(),
        ] {
            assert!(m.text.contains(&want), "{want} not in\n{}", m.text);
        }
        assert!(
            m.text.contains(&format!("{}\r\nvisitors:", fp(6)))
                || m.text.contains(&format!("{}\nvisitors:", fp(6))),
            "{}",
            m.text
        );
        assert_eq!(back(&m.text, &m).unwrap(), text);
    }
}

#[test]
fn a_json_file_keeps_valid_json_with_a_quoted_placeholder() {
    let ids = Ids::default();
    let s = Secret::generate();
    let text = format!(
        "{{\n  \"listen\": 7835,\n  \"clients\": {{\"home\": {{\"secret\": \"{}\"}}, \"odd\": {{\"secret\": 12345}}}}\n}}\n",
        s.encode()
    );
    let m = masked(&text, true, &ids);
    let parsed: serde_json::Value = serde_json::from_str(&m.text).unwrap();
    assert_eq!(
        parsed["clients"]["home"]["secret"],
        placeholder(&s.fingerprint())
    );
    assert!(!m.text.contains("12345"), "{}", m.text);
    let json = unmask(named("server.yaml", true), &m.text, &table(&m))
        .unwrap()
        .0;
    assert_eq!(json, text);
}

#[test]
fn a_placeholder_quoted_another_way_brings_its_secret_back_as_a_string() {
    let ids = Ids::default();
    let st = styles();
    let m = masked(&st.text, false, &ids);
    let p = placeholder(&st.fps[1]);
    let sent = m
        .text
        .replace(&format!("secret: {p}"), &format!("secret: '{p}'"));
    let got = back(&sent, &m).unwrap();
    let s1 = Secret::parse(&st.hidden[1]).unwrap().encode();
    assert!(
        got.contains(&format!("secret: \"{s1}\" # rotated")),
        "{got}"
    );
}

#[test]
fn reordered_list_items_keep_their_secrets() {
    let ids = Ids::default();
    let (a, b) = (Secret::generate(), Secret::generate());
    let text = format!(
        "clients:\n  home:\n    secret:\n      - {}\n      - {}\n",
        a.encode(),
        b.encode()
    );
    let m = masked(&text, false, &ids);
    let (pa, pb) = (placeholder(&a.fingerprint()), placeholder(&b.fingerprint()));
    let sent = m
        .text
        .replace(&pa, "@A@")
        .replace(&pb, &pa)
        .replace("@A@", &pb);
    let got = back(&sent, &m).unwrap();
    assert_eq!(
        got,
        format!(
            "clients:\n  home:\n    secret:\n      - {}\n      - {}\n",
            b.encode(),
            a.encode()
        )
    );
}

fn refused(sent: &str, from: &Masked) -> String {
    let problems = back(sent, from).unwrap_err();
    assert!(
        problems
            .iter()
            .all(|d| d.code == burrow_admin_proto::codes::SECRET_PLACEHOLDER)
    );
    assert!(
        problems
            .iter()
            .all(|d| d.file.as_deref() == Some("server.yaml") && d.line.is_some())
    );
    problems
        .into_iter()
        .map(|d| d.message)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_placeholder_anywhere_but_its_own_field_is_refused_and_named() {
    let ids = Ids::default();
    let st = styles();
    let m = masked(&st.text, false, &ids);
    let home = placeholder(&st.fps[1]);
    let at = m.text.find("  ops:\n    secret: ").unwrap();
    let (r, _) = burrow_admin_proto::text_edit::placeholders_in(&m.text[at..])[0].clone();
    let mut moved = m.text.clone();
    moved.replace_range(at + r.start..at + r.end, &home);
    let msg = refused(&moved, &m);
    assert!(
        msg.contains("visitors.ops.secret")
            && msg.contains("stands for the secret of clients.home.secret"),
        "{msg}"
    );
    let unknown = m.text.replace(&home, &placeholder("0badf00d"));
    let msg = refused(&unknown, &m);
    assert!(
        msg.contains(
            "«secret 0badf00d» stands for no current secret (the server may have restarted: reload the text), or type the secret"
        ),
        "{msg}"
    );
    let partial = m
        .text
        .replace(&format!("{home} # rotated"), &format!("\"x{home}\""));
    assert!(refused(&partial, &m).contains("stands for a whole value"));
    let glued = m
        .text
        .replace(&format!("{home} # rotated"), &format!("{home}Z # rotated"));
    assert!(refused(&glued, &m).contains("stands for a whole value"));
    let as_key = format!("{}\n{home}: 1\n", m.text);
    assert!(refused(&as_key, &m).contains("cannot be a key"));
    let other_field = m
        .text
        .replace("max_connections: 3", &format!("max_connections: {home}"));
    assert!(refused(&other_field, &m).contains("clients.flow.max_connections"));
    let comment = m
        .text
        .replace("# tcp", &format!("# {}", placeholder("0badf00d")));
    assert!(
        refused(&comment, &m).contains("in a comment stands for no secret in this file's comments")
    );
}

#[test]
fn a_comment_placeholder_may_move_to_another_comment() {
    let ids = Ids::default();
    let st = styles();
    let m = masked(&st.text, false, &ids);
    let fp0 = placeholder(&st.fps[0]);
    let sent = m
        .text
        .replace(&format!("# the old key was {fp0}\n"), "")
        .replace("# tcp", &format!("# tcp, was {fp0}"));
    let got = back(&sent, &m).unwrap();
    assert!(
        got.contains(&format!("# tcp, was {}", st.hidden[0])),
        "{got}"
    );
}

#[test]
fn ids_of_burrow_secrets_are_their_fingerprints_and_others_reveal_nothing() {
    let (one, two) = (Ids::default(), Ids::default());
    let at = super::spots::Place::Field(vec!["clients".into(), "a".into(), "secret".into()]);
    let s = Secret::generate();
    assert_eq!(one.of(&at, &s.encode()), s.fingerprint());
    assert_eq!(one.of(&at, "hunter2"), one.of(&at, "hunter2"));
    assert_ne!(one.of(&at, "hunter2"), two.of(&at, "hunter2"));
    assert!(one.of(&at, "hunter2").len() == 8);
}

fn reads(text: &str, path: &[&str]) -> serde_json::Value {
    let tree = crate::config::tree::parse(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    burrow_admin_proto::value::get(&tree.to_json(), &path)
        .cloned()
        .unwrap_or_else(|| panic!("nothing at {path:?} in\n{text}"))
}

#[test]
fn a_restored_secret_reads_back_as_the_same_value_wherever_it_now_stands() {
    let ids = Ids::default();
    let site = "services:\n  web:\n    client: home\n    target: 1\n    web:\n      hosts: [a.example.com]\n";
    let header = "Digest username=bob, response=LEAKPART9";
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";
    let mut cases: Vec<(String, String, Vec<&str>, serde_json::Value)> = vec![
        (
            format!("{site}      request_headers:\n        Authorization: {header}\n"),
            format!("{site}      request_headers: {{ Authorization: @ }}\n"),
            vec!["services", "web", "web", "request_headers", "Authorization"],
            serde_json::json!(header),
        ),
        (
            format!("{site}      basic_auth:\n        bob: {hash}\n"),
            format!("{site}      basic_auth: {{ bob: @ }}\n"),
            vec!["services", "web", "web", "basic_auth", "bob"],
            serde_json::json!(hash),
        ),
    ];
    let one = |v: &str| serde_json::json!(v);
    for (written, value) in [
        ("hunter2,LEAKC7", "hunter2,LEAKC7"),
        ("p#ss:w]rd}", "p#ss:w]rd}"),
        ("part one\n      part two", "part one part two"),
        ("-x, ?y", "-x, ?y"),
    ] {
        let original = format!("clients:\n  home:\n    secret: {written}\n");
        for layout in [
            "clients:\n  home: { secret: @ }\n",
            "clients:\n  home: { secret: [@] }\n",
            "clients:\n  home:\n    secret: [@, x2]\n",
        ] {
            let path = if layout.contains('[') {
                vec!["clients", "home", "secret", "0"]
            } else {
                vec!["clients", "home", "secret"]
            };
            cases.push((original.clone(), layout.into(), path, one(value)));
        }
    }
    for lead in ["&", "*", "!", "%", "@", "`", "- ", "? "] {
        for quote in ['"', '\''] {
            let original =
                format!("clients:\n  home:\n    secret: {quote}{lead}abc, LEAKQ{quote}\n");
            for layout in [
                "clients:\n  home: { secret: @ }\n",
                "clients:\n  home:\n    secret: @\n",
            ] {
                let value = one(&format!("{lead}abc, LEAKQ"));
                cases.push((
                    original.clone(),
                    layout.into(),
                    vec!["clients", "home", "secret"],
                    value,
                ));
            }
        }
    }
    for (original, layout, path, want) in cases {
        let m = masked(&original, false, &ids);
        let found = burrow_admin_proto::text_edit::placeholders_in(&m.text);
        assert_eq!(found.len(), 1, "{}", m.text);
        let p = m.text[found[0].0.clone()].to_string();
        let sent = layout.replace('@', &p);
        let got = back(&sent, &m).unwrap_or_else(|e| panic!("{sent}: {e:#?}"));
        assert_eq!(
            reads(&got, &path),
            want,
            "from\n{original}sent\n{sent}got\n{got}"
        );
    }
}

#[test]
fn a_scalar_secret_turned_into_a_list_keeps_it_and_back() {
    let ids = Ids::default();
    let text = "clients:\n  home:\n    secret: hunter2-x\n";
    let m = masked(text, false, &ids);
    let p = &m.text[burrow_admin_proto::text_edit::placeholders_in(&m.text)[0]
        .0
        .clone()];
    let sent = m.text.replace(p, &format!("[{p}, \"n3w-secret\"]"));
    let got = back(&sent, &m).unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(
        reads(&got, &["clients", "home", "secret"]),
        serde_json::json!(["hunter2-x", "n3w-secret"])
    );
    let list = "clients:\n  home:\n    secret:\n      - old-one-1\n      - new-one-2\n";
    let m = masked(list, false, &ids);
    let found = burrow_admin_proto::text_edit::placeholders_in(&m.text);
    let second = m.text[found[1].0.clone()].to_string();
    let sent = format!("clients:\n  home:\n    secret: {second}\n");
    let got = back(&sent, &m).unwrap_or_else(|e| panic!("{e:#?}"));
    assert_eq!(
        reads(&got, &["clients", "home", "secret"]),
        serde_json::json!("new-one-2")
    );
}

#[test]
fn other_values_get_ids_of_their_own_field_and_burrow_secrets_their_fingerprint() {
    let ids = Ids::default();
    let s = Secret::generate();
    let text = format!(
        "clients:\n  a:\n    secret: hunter2-same\n  b:\n    secret: hunter2-same\n  c:\n    secret: {e}\n  d:\n    secret: {e}\n",
        e = s.encode()
    );
    let m = masked(&text, false, &ids);
    let found: Vec<&str> = burrow_admin_proto::text_edit::placeholders_in(&m.text)
        .into_iter()
        .map(|(_, id)| id)
        .collect();
    assert_ne!(found[0], found[1], "{}", m.text);
    assert_eq!(found[2], s.fingerprint());
    assert_eq!(found[3], s.fingerprint());
    assert_eq!(back(&m.text, &m).unwrap(), text);
}

#[test]
fn a_yaml_only_secret_moved_into_a_json_file_comes_back_as_json() {
    let ids = Ids::default();
    let drop_in = "clients:\n  odd:\n    secret: \"hun\\x74er2\\e\"\n";
    let m = mask::mask(named("server.d/a.yaml", false), drop_in, &root(), &ids).unwrap();
    let p = &m.text[burrow_admin_proto::text_edit::placeholders_in(&m.text)[0]
        .0
        .clone()];
    let sent = format!("{{\"clients\": {{\"odd\": {{\"secret\": \"{p}\"}}}}}}\n");
    let (got, _) = unmask(named("server.json", true), &sent, &table(&m)).unwrap();
    let v: serde_json::Value = serde_json::from_str(&got).unwrap_or_else(|e| panic!("{got}: {e}"));
    assert_eq!(v["clients"]["odd"]["secret"], "hunter2\u{1b}");
}

#[test]
fn a_comment_placeholder_stays_in_its_own_file() {
    let ids = Ids::default();
    let st = styles();
    let m = masked(&st.text, false, &ids);
    let fp0 = placeholder(&st.fps[0]);
    let other = format!("# moved here: {fp0}\nservices: {{}}\n");
    let problems = unmask(named("server.d/new.yaml", false), &other, &table(&m)).unwrap_err();
    assert!(
        problems[0]
            .message
            .contains("in a comment stands for no secret in this file's comments"),
        "{:?}",
        problems[0].message
    );
}

#[test]
fn two_secrets_with_one_fingerprint_in_one_field_are_never_swapped() {
    let mut ids = Ids::default();
    ids.same_fingerprint = true;
    let (a, b) = (Secret::generate().encode(), Secret::generate().encode());
    let text = format!(
        "listen: 7835\nclients:\n  - name: one\n    secret: {a}\n  - name: two\n    secret: {b}\nservices:\n  web: {{ client: one, target: 3000 }}\n"
    );
    let m = masked(&text, false, &ids);
    let ids_seen: Vec<&str> = m.text.split("«secret ").skip(1).map(|r| &r[..8]).collect();
    assert_eq!(ids_seen.len(), 2, "{}", m.text);
    assert_ne!(ids_seen[0], ids_seen[1], "{}", m.text);
    assert_eq!(back(&m.text, &m).unwrap(), text);
}
