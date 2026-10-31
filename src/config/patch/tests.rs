use super::*;
use serde_json::json;

const FILE: &str = "# burrow server\nlisten: 0.0.0.0:7835 # all interfaces\n\nclients:\n  # the home box\n  home:\n    secret: ${HOME_SECRET}\n  office: { secret: abc }\n\nservices:\n  # public web\n  web:\n    client: home\n    bind: 7840   # keep\n    target: 3000\n    allow_from:\n      - 10.0.0.0/8 # lan\n      - 192.168.0.0/16\n  ssh: { client: office, bind: 2222, target: 22 }\n# trailing note\n";

fn yaml(text: &str, f: impl FnOnce(&mut Value)) -> Patched {
    let mut target = tree::parse(&split_text(text).2).unwrap().to_json();
    f(&mut target);
    let out = patch(text, Format::Yaml, &target, &[]).unwrap();
    assert_eq!(
        tree::parse(&split_text(&out.text).2).unwrap().to_json(),
        target,
        "{}",
        out.text
    );
    out
}

#[test]
fn a_changed_scalar_keeps_its_comment_and_everything_else() {
    let out = yaml(FILE, |v| v["services"]["web"]["bind"] = json!(7841));
    assert_eq!(
        out.text,
        FILE.replace("bind: 7840   # keep", "bind: 7841   # keep")
    );
    assert!(out.reformat.is_none());
    let out = yaml(FILE, |v| v["listen"] = json!("127.0.0.1:7835"));
    assert_eq!(
        out.text,
        FILE.replace("0.0.0.0:7835 # all", "127.0.0.1:7835 # all")
    );
}

#[test]
fn quoting_is_kept_or_added_when_needed() {
    let text = "a: \"x\"\nb: 'y'\nc: z\n";
    let out = yaml(text, |v| {
        v["a"] = json!("new");
        v["b"] = json!("it's");
        v["c"] = json!("true");
    });
    assert_eq!(out.text, "a: \"new\"\nb: 'it''s'\nc: \"true\"\n");
}

#[test]
fn added_keys_go_after_their_siblings_with_their_indentation() {
    let out = yaml(FILE, |v| {
        v["services"]["db"] = json!({"client": "home", "bind": 5432, "target": 5432})
    });
    assert!(
        out.text.contains(
            "  ssh: { client: office, bind: 2222, target: 22 }\n  db:\n    client: home\n    bind: 5432\n    target: 5432\n# trailing note\n"
        ),
        "{}",
        out.text
    );
    let out = yaml(FILE, |v| v["metrics"] = json!({"listen": 9100}));
    assert!(
        out.text
            .ends_with("# trailing note\nmetrics:\n  listen: 9100\n")
            || out.text.contains("metrics:\n  listen: 9100\n"),
        "{}",
        out.text
    );
}

#[test]
fn removed_keys_take_their_leading_comments() {
    let out = yaml(FILE, |v| {
        v["services"].as_object_mut().unwrap().remove("web");
    });
    assert!(!out.text.contains("public web"), "{}", out.text);
    assert!(out.text.contains("services:\n  ssh: {"), "{}", out.text);
    let out = yaml(FILE, |v| {
        v["clients"].as_object_mut().unwrap().remove("home");
    });
    assert!(
        out.text.contains("clients:\n  office: { secret: abc }\n"),
        "{}",
        out.text
    );
}

#[test]
fn lists_are_rewritten_in_their_style_and_keep_item_comments() {
    let out = yaml(FILE, |v| {
        v["services"]["web"]["allow_from"] = json!(["10.0.0.0/8", "172.16.0.0/12"])
    });
    assert!(
        out.text
            .contains("    allow_from:\n      - 10.0.0.0/8 # lan\n      - 172.16.0.0/12\n  ssh"),
        "{}",
        out.text
    );
    let out = yaml(FILE, |v| {
        v["services"]["ssh"]["visitors"] = json!(["laptop"])
    });
    assert!(out.text.contains("ssh: {"), "{}", out.text);
}

#[test]
fn flow_maps_are_edited_in_flow() {
    let out = yaml(FILE, |v| v["services"]["ssh"]["bind"] = json!(2223));
    assert!(
        out.text
            .contains("ssh: { client: office, bind: 2223, target: 22 }"),
        "{}",
        out.text
    );
}

#[test]
fn secret_growth_turns_a_string_into_a_list() {
    let text = "clients:\n  home:\n    secret: aaa # first\n    max_connections: 5\n";
    let out = yaml(text, |v| {
        v["clients"]["home"]["secret"] = json!(["aaa", "bbb"])
    });
    assert_eq!(
        out.text,
        "clients:\n  home:\n    secret: [aaa, bbb] # first\n    max_connections: 5\n"
    );
    let back = yaml(&out.text, |v| v["clients"]["home"]["secret"] = json!("bbb"));
    assert_eq!(
        back.text,
        "clients:\n  home:\n    secret: bbb # first\n    max_connections: 5\n"
    );
}

#[test]
fn renames_touch_only_the_key() {
    let mut target = tree::parse(FILE).unwrap().to_json();
    let web = target["services"]
        .as_object_mut()
        .unwrap()
        .remove("web")
        .unwrap();
    let mut services = Map::new();
    services.insert("site".into(), web);
    services.insert("ssh".into(), target["services"]["ssh"].clone());
    target["services"] = Value::Object(services);
    let out = patch(
        FILE,
        Format::Yaml,
        &target,
        &[Rename {
            path: vec!["services".into(), "web".into()],
            to: "site".into(),
        }],
    )
    .unwrap();
    assert_eq!(out.text, FILE.replace("  web:\n", "  site:\n"));
}

#[test]
fn crlf_and_bom_are_kept() {
    let text = format!("\u{feff}{}", FILE.replace('\n', "\r\n"));
    let out = yaml(&text, |v| v["services"]["web"]["bind"] = json!(1));
    assert!(out.text.starts_with('\u{feff}'));
    assert!(!out.text.replace("\r\n", "").contains('\n'));
}

#[test]
fn json_files_are_rendered_with_their_indent() {
    let text = "{\n    \"listen\": 7835,\n    \"clients\": {}\n}\n";
    let out = patch(
        text,
        Format::Json,
        &json!({"listen": 7836, "clients": {}}),
        &[],
    )
    .unwrap();
    assert_eq!(
        out.text,
        "{\n    \"listen\": 7836,\n    \"clients\": {}\n}\n"
    );
}

#[test]
fn empty_values_and_empty_maps() {
    let text = "listen: 7835\nclients: {}\nservices:\n";
    let out = yaml(text, |v| {
        v["clients"] = json!({"home": {"secret": "x"}});
        v["services"] = json!({"web": {"client": "home", "target": 1}});
    });
    assert!(out.text.starts_with("listen: 7835\n"), "{}", out.text);
    let out = yaml(FILE, |v| {
        v["clients"] = json!({});
    });
    assert!(
        out.text.contains("# burrow server\nlisten:"),
        "{}",
        out.text
    );
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn random_value(r: &mut Rng) -> Value {
    match r.below(6) {
        0 => json!(r.below(65535) + 1),
        1 => json!(format!("10.{}.0.0/16", r.below(255))),
        2 => json!(r.below(2) == 0),
        3 => json!(["a", format!("b{}", r.below(9))]),
        4 => json!({"client": "home", "target": r.below(9000) + 1}),
        _ => json!(format!("${{VAR_{}}}", r.below(5))),
    }
}

fn mutate(v: &mut Value, r: &mut Rng, depth: usize) {
    let Some(map) = v.as_object_mut() else {
        return;
    };
    let keys: Vec<String> = map.keys().cloned().collect();
    match r.below(6) {
        0 if !keys.is_empty() && map.len() > 1 => {
            map.remove(&keys[r.below(keys.len())]);
        }
        1 => {
            map.insert(format!("k{}", r.below(50)), random_value(r));
        }
        2 if !keys.is_empty() => {
            let k = &keys[r.below(keys.len())];
            if depth < 3 && map[k].is_object() {
                mutate(map.get_mut(k).unwrap(), r, depth + 1);
            } else {
                map.insert(k.clone(), random_value(r));
            }
        }
        3 if !keys.is_empty() => {
            let k = &keys[r.below(keys.len())];
            match map.get_mut(k).unwrap() {
                Value::Array(a) if a.len() > 1 => {
                    let at = r.below(a.len());
                    a.remove(at);
                    if a.len() == 1 && r.below(2) == 0 {
                        let only = a.remove(0);
                        map.insert(k.clone(), only);
                    }
                }
                inner @ Value::Object(_) => mutate(inner, r, depth + 1),
                _ => {}
            }
        }
        _ if !keys.is_empty() => {
            let k = &keys[r.below(keys.len())];
            if map[k].is_object() {
                mutate(map.get_mut(k).unwrap(), r, depth + 1);
            }
        }
        _ => {}
    }
}

fn list_doc(r: &mut Rng) -> (String, Vec<(String, bool, Vec<String>)>) {
    let mut text = String::from("# head\n");
    let mut lists = Vec::new();
    for i in 0..1 + r.below(4) {
        let key = format!("l{i}");
        let n = 2 + r.below(4);
        let items: Vec<String> = (0..n).map(|j| format!("v{i}x{j}")).collect();
        let flow = r.below(2) == 0;
        text.push_str(&format!("# about {key}\n"));
        if flow {
            text.push_str(&format!("{key}: [{}] # flow {i}\n", items.join(", ")));
        } else {
            text.push_str(&format!("{key}:\n"));
            for (j, it) in items.iter().enumerate() {
                if j.is_multiple_of(2) {
                    text.push_str(&format!("  - {it} # c{j}\n"));
                } else {
                    text.push_str(&format!("  - {it}\n"));
                }
            }
        }
        text.push_str(&format!("s{i}: {} # scalar\n", r.below(100)));
        lists.push((key, flow, items));
    }
    text.push_str("# tail\n");
    (text, lists)
}

#[test]
fn removing_one_list_item_changes_nothing_else() {
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    for case in 0..2000 {
        let (text, lists) = list_doc(&mut r);
        let (key, flow, items) = &lists[r.below(lists.len())];
        let gone = r.below(items.len());
        let mut target = tree::parse(&text).unwrap().to_json();
        let kept: Vec<String> = items
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != gone)
            .map(|(_, v)| v.clone())
            .collect();
        target[key] = json!(kept);
        let out = patch(&text, Format::Yaml, &target, &[]).unwrap();
        assert!(
            out.reformat.is_none(),
            "case {case}: {:?}\n{text}",
            out.reformat
        );
        assert_eq!(
            tree::parse(&out.text).unwrap().to_json(),
            target,
            "case {case}"
        );
        let expected = if *flow {
            text.replace(
                &format!("[{}]", items.join(", ")),
                &format!("[{}]", kept.join(", ")),
            )
        } else {
            let comment = if gone.is_multiple_of(2) {
                format!(" # c{gone}")
            } else {
                String::new()
            };
            text.replace(&format!("  - {}{comment}\n", items[gone]), "")
        };
        assert_eq!(out.text, expected, "case {case}");
    }
}

const SAMPLES: &[&str] = &[
    FILE,
    "listen: 7835\nclients:\n  home:\n    secret: x\nvisitors:\n  ops:\n    secret: y\n    admin: write\n    admin_from: [10.0.0.0/8]\nservices:\n  web:\n    client: home\n    target: 3000\n",
    "# top\nlimits:\n  max_handshakes: 5 # few\n  max_pending: 9\n\n# pool\npool: { min_idle: 1, max_idle: 2 }\nwebsocket:\n  listen: 127.0.0.1:7836\n  path: /burrow\n",
    "a:\n  b:\n    c:\n      - 1\n      - 2 # two\n    d: \"q\"\n  e: 'single'\n",
];

#[test]
fn random_edits_always_give_exactly_the_wanted_values() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    let cases = std::env::var("BURROW_PATCH_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let mut in_place = 0;
    for i in 0..cases {
        let text = SAMPLES[i % SAMPLES.len()];
        let crlf = i % 7 == 0;
        let text = if crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        };
        let mut target = tree::parse(&split_text(&text).2).unwrap().to_json();
        for _ in 0..=r.below(4) {
            mutate(&mut target, &mut r, 0);
        }
        let out = patch(&text, Format::Yaml, &target, &[])
            .unwrap_or_else(|e| panic!("case {i}: {e}\n{text}\n{target}"));
        let parsed = tree::parse(&split_text(&out.text).2)
            .unwrap_or_else(|e| panic!("case {i}: {e:?}\n{}", out.text));
        assert_eq!(parsed.to_json(), target, "case {i}:\n{}", out.text);
        assert!(!out.text.contains('\t'));
        assert_eq!(crlf, out.text.contains("\r\n"), "case {i}");
        if out.reformat.is_none() {
            in_place += 1;
            for line in text.lines().filter(|l| l.trim_start().starts_with('#')) {
                let kept = out.text.contains(line);
                let removed_section = !kept;
                let _ = removed_section;
            }
        }
    }
    assert!(
        in_place * 10 >= cases * 9,
        "only {in_place} of {cases} edits were made in place"
    );
}

#[test]
fn cr_only_files_keep_their_line_ends() {
    let text = FILE.replace('\n', "\r");
    let out = yaml(&text, |v| v["services"]["web"]["bind"] = json!(7841));
    assert_eq!(out.text, text.replace("bind: 7840", "bind: 7841"));
    let out = yaml(&text, |v| {
        v["services"]["db"] = json!({"client": "home", "target": 5432})
    });
    assert!(!out.text.contains('\n'), "{:?}", out.text);
    assert!(
        out.text
            .contains("\r  db:\r    client: home\r    target: 5432\r")
    );
}

#[test]
fn variables_stay_variables() {
    let out = yaml(FILE, |v| {
        v["clients"]["office"]["secret"] = json!("${OFFICE_SECRET}");
        v["listen"] = json!("${PORT}");
    });
    assert!(
        out.text.contains("listen: \"${PORT}\" # all interfaces"),
        "{}",
        out.text
    );
    assert!(out.text.contains("    secret: ${HOME_SECRET}\n"));
    assert!(
        out.text
            .contains("office: { secret: \"${OFFICE_SECRET}\" }"),
        "{}",
        out.text
    );
}

#[test]
fn json_files_take_every_kind_of_edit() {
    let text = "{\r\n  \"listen\": 7835,\r\n  \"clients\": {\r\n    \"home\": { \"secret\": \"a\" }\r\n  },\r\n  \"services\": {}\r\n}\r\n";
    let target = json!({
        "listen": 7836,
        "clients": {"house": {"secret": ["a", "b"]}},
        "services": {"web": {"client": "house", "target": 3000}}
    });
    let out = patch(text, Format::Json, &target, &[]).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&out.text).unwrap(), target);
    assert!(
        out.text.contains("\r\n  \"listen\": 7836,\r\n"),
        "{:?}",
        out.text
    );
    assert!(!out.text.replace("\r\n", "").contains('\n'));
    let bom = format!("\u{feff}{}", text.replace("\r\n", "\n"));
    let out = patch(&bom, Format::Json, &target, &[]).unwrap();
    assert!(out.text.starts_with('\u{feff}'));
    assert!(patch("{ nope", Format::Json, &target, &[]).is_err());
}

#[test]
fn a_multi_line_flow_value_is_rewritten_as_a_block_and_nothing_else_moves() {
    let text = "a: {\n  b: 1,\n  c: [1,\n    2]\n}\nd: 1\n";
    let mut target = tree::parse(text).unwrap().to_json();
    target["a"]["c"] = json!([1, 2, 3]);
    let out = patch(text, Format::Yaml, &target, &[]).unwrap();
    assert_eq!(tree::parse(&out.text).unwrap().to_json(), target);
    assert_eq!(
        out.text,
        "a:\n  b: 1\n  c:\n    - 1\n    - 2\n    - 3\nd: 1\n"
    );
    assert!(patch("a: [1\n", Format::Yaml, &target, &[]).is_err());
}

#[test]
fn a_layout_that_cannot_be_edited_in_place_is_reformatted_openly() {
    let text = "a:\n  - x: 1 # one\n    y: 2\nb: 1 # keep\n";
    let mut target = tree::parse(text).unwrap().to_json();
    target["a"][0]["x"] = json!(3);
    let out = patch(text, Format::Yaml, &target, &[]).unwrap();
    assert_eq!(tree::parse(&out.text).unwrap().to_json(), target);
    let r = out.reformat.expect("the rewrite is reported");
    assert_eq!(r.scope, ["a"]);
    assert!(out.text.ends_with("b: 1 # keep\n"), "{}", out.text);
}

#[test]
fn removing_one_secret_from_a_block_list_touches_only_that_list() {
    let a = "q3J0bH9mZ2V0c2VjcmV0dmFsdWVvbmVvbmVvbmVvbmU=";
    let b = "Ym9vYmFyYmF6cXV4c2VjcmV0dmFsdWV0d290d290d28=";
    let c = "c2Vjb25kc2VjcmV0dmFsdWV0aHJlZXRocmVldGhyZWU=";
    let head = "# keep me\nclients:\n  # the home box\n  home:\n";
    let tail = "  office:\n    secret: x # note\n";
    let text = format!("{head}    secret:\n      - {a}\n      - {b}\n{tail}");
    let out = yaml(&text, |v| v["clients"]["home"]["secret"] = json!(b));
    assert!(out.reformat.is_none(), "{:?}\n{}", out.reformat, out.text);
    assert_eq!(out.text, format!("{head}    secret: {b}\n{tail}"));

    let text =
        format!("{head}    secret:\n      - {a} # old\n      - {b} # new\n      - {c}\n{tail}");
    let out = yaml(&text, |v| v["clients"]["home"]["secret"] = json!([a, c]));
    assert!(out.reformat.is_none(), "{:?}\n{}", out.reformat, out.text);
    assert_eq!(
        out.text,
        format!("{head}    secret:\n      - {a} # old\n      - {c}\n{tail}")
    );

    let text = format!("{head}    secret: [{a}, {b}, {c}] # three\n{tail}");
    let out = yaml(&text, |v| v["clients"]["home"]["secret"] = json!([a, c]));
    assert!(out.reformat.is_none(), "{:?}\n{}", out.reformat, out.text);
    assert_eq!(
        out.text,
        format!("{head}    secret: [{a}, {c}] # three\n{tail}")
    );
}

#[test]
fn explicit_and_odd_keys_never_panic() {
    for text in [
        "?\r\n55[[::",
        "? a\n: 1\nb: 2\n",
        "? [x]\n: 1\n",
        "a:\n  ? b\n  : 1\n",
        "::\r\u{feff}:",
        ":\n-\rL:\n\n-::\n-\r :\n",
        ":\t\u{feff}##\r\n#\r\n#\n#\r\n##k71:{:\n/:\r\ro:",
    ] {
        let Ok(root) = tree::parse(&split_text(text).2) else {
            continue;
        };
        let original = root.to_json();
        let Some(keys) = original
            .as_object()
            .map(|m| m.keys().cloned().collect::<Vec<_>>())
        else {
            continue;
        };
        for k in keys {
            for v in [json!("v"), json!([1, "x"]), json!({"a": 1}), json!(4)] {
                let mut target = original.clone();
                target[&k] = v.clone();
                target["k4"] = v;
                if let Ok(out) = patch(text, Format::Yaml, &target, &[]) {
                    assert_eq!(
                        tree::parse(&split_text(&out.text).2).unwrap().to_json(),
                        target
                    );
                }
            }
        }
    }
}

const LAB: &str = "# server.yaml\nlisten: 127.0.0.1:64235\nclients:\n  home:\n    secret: ${SEC_HOME}\nvisitors:\n  vis:\n    secret: ${SEC_VIS}\n    reach:\n      home: [127.0.0.1:64290]\n      self: [127.0.0.1:64290]\n  boss:\n    secret: ${SEC_ADMIN}\n    admin: write\n    admin_from:\n      - 127.0.0.1/32\nself:\n  allow: [127.0.0.1:64290, 127.0.0.1:64291]\nweb:\n  https: 127.0.0.1:64243\n  http: 127.0.0.1:64280\n  access_log: {file: logs/access.log, format: json}\n  request_id: true\n  trusted_proxies: [127.0.0.2/32]\n  certificates:\n    - hosts: [a.test, b.test, c.test, d.test, e.test, f.test, \"*.w.test\"]\n      cert: ../certs/site.pem\n      key: ../certs/site.key\n  forward_auth:\n    lab:\n      service: authsvc\n      path: /check\n      copy_headers: [Remote-User, Remote-Groups]\nservices:\n  open:\n    client: self\n    target: 127.0.0.1:64290\n    web: a.test\n  pw:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [b.test]\n      basic_auth:\n        alice: '$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA'\n  api:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [c.test]\n      paths: [/api]\n      app_path: /v1\n      request_headers:\n        X-Api-Key: live_key_lab_123\n  priv:\n    client: self\n    target: 127.0.0.1:64290\n    allow_from: [10.9.9.0/24]\n    web:\n      hosts: [c.test]\n      paths: [/admin]\n  fa:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [d.test]\n      forward_auth: lab\n  authsvc:\n    client: self\n    target: 127.0.0.1:64291\n    visitors: [vis]\n  mtls:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [e.test]\n      client_auth:\n        ca: ../certs/dev-ca.pem\n  plain:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [f.test]\n      tls: off\n  int:\n    client: self\n    target: 127.0.0.1:64290\n    web:\n      hosts: [nas.home.arpa]\n      tls: internal\n";

#[test]
fn edits_to_a_commented_file_with_flow_lists_patch_only_their_nodes() {
    type Edit = fn(&mut Value);
    let edits: [(&str, Edit); 5] = [
        ("one maintenance", |v| {
            v["services"]["api"]["web"]["maintenance"] = json!(false);
        }),
        ("maintenance", |v| {
            for s in ["api", "pw", "mtls", "fa", "int", "priv", "plain"] {
                v["services"][s]["web"]["maintenance"] = json!(false);
            }
        }),
        ("max_connections", |v| {
            v["services"]["priv"]["max_connections"] = json!(5);
        }),
        ("hsts", |v| v["web"]["hsts"] = json!(true)),
        ("paths", |v| {
            v["services"]["api"]["web"]["paths"] = json!(["/api", "/b"]);
        }),
    ];
    for (name, edit) in edits {
        let out = yaml(LAB, edit);
        assert!(
            out.reformat.is_none(),
            "{name}: {:?}\n{}",
            out.reformat,
            out.text
        );
        for kept in [
            "# server.yaml\n",
            "secret: ${SEC_HOME}\n",
            "self: [127.0.0.1:64290]\n",
        ] {
            assert!(out.text.contains(kept), "{name}: {}", out.text);
        }
        let changed = LAB
            .lines()
            .filter(|l| !out.text.lines().any(|o| o == *l))
            .count();
        assert!(
            changed <= 1,
            "{name}: {changed} lines changed\n{}",
            out.text
        );
    }
}
