use crate::config::settings::{NotifyVia, SmtpSecurity};
use crate::config::{Format, Loaded, Outcome, Role, load_str};
use burrow_config::NotifyEvent;

const BASE: &str = "listen: 7835\nclients:\n  home: { secret: \"yjgGDKUL5TY/9SCE2jvBsr2z5zLSKl+x1p76T98Vf/U=\" }\nservices:\n  ssh: { client: home, target: 22, bind: 2222 }\n";

fn load(notify: &str, env: &[(&str, &str)]) -> Outcome {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    load_str(
        "server.yaml",
        &format!("{BASE}{notify}"),
        Format::Yaml,
        Some(Role::Server),
        &move |name: &str| env.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()),
    )
}

fn errors(o: &Outcome) -> Vec<String> {
    o.report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect()
}

fn targets(o: Outcome) -> Vec<crate::config::settings::NotifyTarget> {
    assert_eq!(errors(&o), Vec::<String>::new());
    match o.loaded {
        Some(Loaded::Server(s, _)) => s.notify,
        _ => panic!("a server config"),
    }
}

#[test]
fn no_notify_sends_nothing() {
    assert!(targets(load("", &[])).is_empty());
}

#[test]
fn the_three_destinations_and_their_events_are_read() {
    let t = targets(load(
        "notify:\n  - webhook: https://hooks.example.com/burrow\n  - ntfy: https://ntfy.sh/my-topic\n    events: [site_down, site_back]\n  - email: { to: ops@example.com, smtp: \"smtp://alerts@example.com@mail.example.com\", password: \"${SMTP_PASS}\" }\n",
        &[("SMTP_PASS", "hunter2")],
    ));
    assert_eq!(t.len(), 3);
    assert_eq!(
        t[0].via,
        NotifyVia::Webhook("https://hooks.example.com/burrow".into())
    );
    assert!(t[0].events.is_empty());
    assert!(t[1].wants(NotifyEvent::SiteDown) && !t[1].wants(NotifyEvent::ClientOffline));
    let NotifyVia::Email(m) = &t[2].via else {
        panic!("email");
    };
    assert_eq!((m.host.as_str(), m.port), ("mail.example.com", 587));
    assert_eq!(m.security, SmtpSecurity::StartTls);
    assert_eq!(m.user.as_deref(), Some("alerts@example.com"));
    assert_eq!(m.from, "alerts@example.com");
    assert_eq!(m.password.as_ref().unwrap().expose(), "hunter2");
}

#[test]
fn smtps_is_tls_on_465_and_a_local_server_may_skip_starttls() {
    let email = |smtp: &str| {
        let t = targets(load(
            &format!(
                "notify:\n  - email: {{ to: ops@example.com, smtp: \"{smtp}\", from: burrow@example.com }}\n"
            ),
            &[],
        ));
        match &t[0].via {
            NotifyVia::Email(m) => (m.port, m.security),
            _ => panic!("email"),
        }
    };
    assert_eq!(email("smtps://mail.example.com"), (465, SmtpSecurity::Tls));
    assert_eq!(
        email("smtp://mail.example.com:2525"),
        (2525, SmtpSecurity::StartTls)
    );
    assert_eq!(
        email("smtp://localhost:25"),
        (25, SmtpSecurity::StartTlsIfOffered)
    );
}

#[test]
fn each_mistake_is_named_with_its_place() {
    let cases = [
        (
            "notify:\n  - { webhook: \"https://a.example.com\", ntfy: \"https://ntfy.sh/x\" }\n",
            "notify[0]: name exactly one of webhook, ntfy or email",
        ),
        (
            "notify:\n  - webhook: ftp://a.example.com/x\n",
            "notify[0].webhook: the address must start with https://",
        ),
        (
            "notify:\n  - ntfy: http://203.0.113.7/topic\n",
            "notify[0].ntfy: 203.0.113.7 is a public address; plain http:// is only for this machine or the LAN, so use https://",
        ),
        (
            "notify:\n  - webhook: \"https://user:pw@a.example.com/x\"\n",
            "notify[0].webhook: a user:password@ in the address is not supported",
        ),
        (
            "notify:\n  - email: { to: ops, smtp: \"smtp://mail.example.com\", from: a@example.com }\n",
            "notify[0].email.to: \"ops\" is not an email address",
        ),
        (
            "notify:\n  - email: { to: ops@example.com, smtp: \"smtp://bob:pw@mail.example.com\" }\n",
            "notify[0].email.smtp: the password does not go in the address",
        ),
        (
            "notify:\n  - email: { to: ops@example.com, smtp: \"smtp://bob@mail.example.com\" }\n",
            "notify[0].email.from: is needed because the mail server's user name is not an address",
        ),
        (
            "notify:\n  - email: { to: ops@example.com, smtp: \"smtp://a@example.com@mail.example.com\", password: hunter2 }\n",
            "notify[0].email.password is written in the file; use ${SMTP_PASS} or file: so it stays out of the config",
        ),
        (
            "notify:\n  - email: { to: ops@example.com, smtp: \"smtp://mail.example.com\", from: a@example.com, password: \"${P}\" }\n",
            "notify[0].email: a password needs a user name in smtp",
        ),
    ];
    for (text, want) in cases {
        let o = load(text, &[("P", "x")]);
        assert_eq!(errors(&o), [want], "{text}");
    }
    let o = load(
        "notify:\n  - email: { to: a@b.example, smtp: smtp://x.example.com, from: a@b.example }\n  - ntfy: https://ntfy.sh/t\n    events: [client_gone]\n",
        &[],
    );
    assert_eq!(errors(&o).len(), 1, "{:?}", errors(&o));
}

#[test]
fn http_to_the_lan_or_this_machine_is_allowed() {
    let t = targets(load(
        "notify:\n  - webhook: http://192.168.1.20:8123/api/webhook/burrow\n  - ntfy: http://localhost:8080/burrow\n",
        &[],
    ));
    assert_eq!(t.len(), 2);
}

#[test]
fn the_example_in_monitoring_md_is_valid() {
    let doc = include_str!("../../../../docs/monitoring.md");
    let section = &doc[doc.find("## Notifications").unwrap()..];
    let blocks = crate::config::check::web::tests_docs::yaml_blocks(section);
    let block = blocks.first().expect("a yaml block under Notifications");
    let t = targets(load(block, &[("SMTP_PASS", "x")]));
    assert_eq!(t.len(), 3);
}
