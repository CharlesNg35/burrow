use super::mask::{self, Ids, Masked, Table};
use super::unmask::unmask;
use crate::wire::secret::Secret;
use burrow_admin_proto::schema::Node;
use burrow_admin_proto::text_edit::placeholder;

fn root() -> Node {
    crate::config::admin_schema::server_schema().root
}

fn masked(text: &str, json: bool, ids: &Ids) -> Masked {
    mask::mask(text, json, &root(), ids).unwrap()
}

fn table(m: &Masked) -> Table {
    m.kept.iter().cloned().rev().collect()
}

fn back(sent: &str, from: &Masked) -> Result<String, Vec<burrow_admin_proto::diag::Diag>> {
    unmask("server.yaml", sent, &table(from)).map(|(t, _)| t)
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
    assert_eq!(back(&m.text, &m).unwrap(), text);
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
        msg.contains("«secret 0badf00d» stands for no current secret"),
        "{msg}"
    );
    let partial = m
        .text
        .replace(&format!("{home} # rotated"), &format!("\"x{home}\""));
    assert!(refused(&partial, &m).contains("stands for a whole value"));
    let as_key = format!("{}\n{home}: 1\n", m.text);
    assert!(refused(&as_key, &m).contains("cannot be a key"));
    let other_field = m
        .text
        .replace("max_connections: 3", &format!("max_connections: {home}"));
    assert!(refused(&other_field, &m).contains("clients.flow.max_connections"));
    let comment = m
        .text
        .replace("# tcp", &format!("# {}", placeholder("0badf00d")));
    assert!(refused(&comment, &m).contains("in a comment stands for no current secret"));
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
    let s = Secret::generate();
    assert_eq!(one.of(&s.encode()), s.fingerprint());
    assert_eq!(one.of("hunter2"), one.of("hunter2"));
    assert_ne!(one.of("hunter2"), two.of("hunter2"));
    assert!(one.of("hunter2").len() == 8);
}
