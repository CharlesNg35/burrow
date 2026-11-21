use burrow_admin_proto::msg::{AdminLevel, State, req};
use burrow_admin_proto::view::{ConfigView, Shown, ShownRequest};
use serde_json::json;

use super::{World, core_running, session, world};
use crate::admin::core::{Core, dispatch, values};
use crate::wire::secret::Secret;

const HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";
const LITERALS: [&str; 6] = [
    HASH,
    "cf-literal-token-5e1f",
    "dHNpZy1zZWNyZXQtdmFsdWU=",
    "acme-dns-literal-pass-77",
    "c2VjcmV0LWhlYWRlcg",
    "live_key_header_9q8w",
];
const ENV_TOKEN: &str = "cf-env-token-from-env-file-31";
const ENV_PASS: &str = "acme-dns-env-pass-from-env-file-42";
const ENV_HEADER: &str = "upstream-env-header-value-53";

pub(super) struct Secrets {
    pub(super) world: World,
    pub(super) office: [Secret; 2],
    pub(super) phone: Secret,
    pub(super) text: String,
}

pub(super) fn every_secret() -> Secrets {
    let world = world();
    let office = [Secret::generate(), Secret::generate()];
    let phone = Secret::generate();
    let clients = format!(
        "clients:\n  office:\n    secret:\n      - {}\n      - {}\n  phone:\n    secret: ${{PHONE_SECRET}}\n",
        office[0].encode(),
        office[1].encode()
    );
    let site = format!(
        "services:\n  site:\n    client: home\n    target: 3000\n    web:\n      hosts: [site.example.com]\n      tls: \"off\"\n      basic_auth:\n        alice: \"{HASH}\"\n      request_headers:\n        Authorization: Basic {}\n        X-Api-Key: {}\n        X-Env-Token: ${{UP_TOKEN}}\n",
        LITERALS[4], LITERALS[5]
    );
    let web = format!(
        "web:\n  http: 7899\n  letsencrypt:\n    email: ops@example.com\n    dns:\n      - domains: [example.com]\n        cloudflare: {{ token: {} }}\n      - domains: [example.net]\n        cloudflare: {{ token: \"${{CF_TOKEN}}\" }}\n      - domains: [example.org]\n        rfc2136: {{ server: \"ns.example.org:53\", key_name: burrow, secret: \"{}\" }}\n      - domains: [example.dev]\n        acme_dns: {{ api: \"https://auth.example.dev\", username: u1, password: {}, subdomain: s1 }}\n      - domains: [example.app]\n        acme_dns: {{ api: \"https://auth.example.app\", username: u2, password: \"${{ACMEDNS_KEY}}\", subdomain: s2 }}\n",
        LITERALS[1], LITERALS[2], LITERALS[3]
    );
    let text = world
        .text
        .replace("clients:\n", &clients)
        .replace(
            "visitors:\n",
            "visitors:\n  laptop:\n    secret: ${LAPTOP_SECRET}\n",
        )
        .replace("services:\n", &site)
        + &web;
    Secrets {
        world,
        office,
        phone,
        text,
    }
}

pub(super) fn core_of(s: &Secrets, laptop: &Secret) -> Core {
    let mut core = core_running(&s.text, &s.world.text);
    let vars = [
        ("PHONE_SECRET", s.phone.encode()),
        ("LAPTOP_SECRET", laptop.encode()),
        ("CF_TOKEN", ENV_TOKEN.to_string()),
        ("ACMEDNS_KEY", ENV_PASS.to_string()),
        ("UP_TOKEN", ENV_HEADER.to_string()),
    ];
    core.env = Box::new(move |name: &str| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.clone())
    });
    core
}

pub(super) fn never_shown(s: &Secrets, laptop: &Secret) -> Vec<String> {
    let mut out: Vec<String> = LITERALS.iter().map(|l| l.to_string()).collect();
    out.extend([ENV_TOKEN, ENV_PASS, ENV_HEADER].map(str::to_string));
    out.extend(
        [
            &s.world.home,
            &s.world.ops,
            &s.office[0],
            &s.office[1],
            &s.phone,
            laptop,
        ]
        .map(Secret::encode),
    );
    out
}

#[test]
fn no_secret_value_and_no_hash_reaches_the_shown_copied_or_downloaded_text() {
    let s = every_secret();
    let laptop = Secret::generate();
    let core = core_of(&s, &laptop);
    let w = &s.world;
    for level in [AdminLevel::Read, AdminLevel::Write] {
        for show in [Shown::Written, Shown::WithDefaults] {
            let reply = dispatch(
                &core,
                &session(w, level),
                req::CONFIG_VIEW,
                &json!(ShownRequest { show }),
            )
            .unwrap();
            let v: ConfigView = serde_json::from_value(reply.clone()).unwrap();
            let text = v.text();
            for secret in never_shown(&s, &laptop) {
                assert!(!text.contains(&secret), "{show:?} shows {secret}");
                assert!(
                    !reply.to_string().contains(&secret),
                    "{show:?} sends {secret}"
                );
            }
            assert!(text.contains(values::HASH_SHOWN), "{text}");
            assert!(text.contains(&format!("«secret {}»", w.home.fingerprint())));
            assert!(text.contains(&format!("«secret {}»", s.office[1].fingerprint())));
            for name in [
                "${PHONE_SECRET}",
                "${LAPTOP_SECRET}",
                "${CF_TOKEN}",
                "${ACMEDNS_KEY}",
                "${UP_TOKEN}",
            ] {
                assert!(text.contains(name), "{name} in {text}");
            }
            assert!(text.contains("site.example.com") && text.contains("ops@example.com"));
        }
    }
}

#[test]
fn get_config_hides_the_same_secrets_the_same_way() {
    let s = every_secret();
    let laptop = Secret::generate();
    let core = core_of(&s, &laptop);
    for show in [Shown::Written, Shown::WithDefaults] {
        let state: State = serde_json::from_value(
            dispatch(
                &core,
                &session(&s.world, AdminLevel::Read),
                req::STATE,
                &json!(ShownRequest { show }),
            )
            .unwrap(),
        )
        .unwrap();
        let file: ConfigView = serde_json::from_value(
            dispatch(
                &core,
                &session(&s.world, AdminLevel::Read),
                req::CONFIG_VIEW,
                &json!(ShownRequest { show }),
            )
            .unwrap(),
        )
        .unwrap();
        let back = crate::config::tree::parse(&file.text()).unwrap().to_json();
        assert_eq!(back, crate::admin::core::view::plain(&state.values));
    }
}
