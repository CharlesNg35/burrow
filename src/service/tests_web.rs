use std::path::PathBuf;

use super::checks::misfits_of;
use super::systemd::Systemd;
use super::*;

fn manifest(web_state: Option<&str>) -> Manifest {
    Manifest {
        name: "burrow-home".into(),
        role: "server".into(),
        config: "/etc/burrow/home/server.yaml".into(),
        source_config: "/srv/home.yaml".into(),
        env_file: None,
        log_file: "/var/log/burrow/home/home.log".into(),
        binary: "/usr/local/bin/burrow".into(),
        copied_binary: true,
        account: "burrow-home".into(),
        created_account: true,
        low_ports: false,
        files: vec![],
        version: "0.1.0".into(),
        admin_writable: false,
        web_state: web_state.map(PathBuf::from),
    }
}

fn load(rest: &str) -> crate::config::Loaded {
    let secret = crate::wire::secret::Secret::generate().encode();
    let text = format!(
        "clients:\n  home: {{ secret: {secret} }}\n{rest}services:\n  nas: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: passthrough }} }}\n"
    );
    let nothing = |_: &str| None;
    let o = crate::config::load_str("c.yaml", &text, crate::config::Format::Yaml, None, &nothing);
    let shown = o
        .report
        .render_text(crate::base::style::Style { color: false });
    o.loaded.unwrap_or_else(|| panic!("{shown}"))
}

#[test]
fn a_service_with_lets_encrypt_gets_a_private_state_folder_from_systemd() {
    let unit = Systemd.render(&manifest(Some("/var/lib/burrow/home")));
    assert!(
        unit.contains("StateDirectory=burrow/home\nStateDirectoryMode=0700\n"),
        "{unit}"
    );
    assert!(
        unit.contains(
            "--log-file /var/log/burrow/home/home.log --state-dir /var/lib/burrow/home\n"
        ),
        "{unit}"
    );
    assert!(unit.contains("ProtectSystem=strict"));
    let plain = Systemd.render(&manifest(None));
    assert!(!plain.contains("StateDirectory") && !plain.contains("--state-dir"));
    assert_eq!(
        crate::install::service_state_dir("home"),
        PathBuf::from("/var/lib/burrow/home")
    );
}

#[test]
fn lets_encrypt_without_the_folder_and_the_plain_web_port_are_named_for_a_reinstall() {
    let le = load("listen: 7835\nweb: { https: 8443, letsencrypt: { email: ops@example.com } }\n");
    let without = misfits_of(&Systemd, &manifest(None), &le);
    assert_eq!(without.len(), 1, "{without:#?}");
    assert!(
        without[0]
            .message
            .contains("without a folder for Let's Encrypt"),
        "{}",
        without[0].message
    );
    assert!(misfits_of(&Systemd, &manifest(Some("/var/lib/burrow/home")), &le).is_empty());
    let mut both = load("listen: 7835\nweb: { https: 443 }\n");
    if let crate::config::Loaded::Server(s, _) = &mut both {
        s.web.as_mut().unwrap().http = Some("[::]:80".parse().unwrap());
    }
    let low = misfits_of(&Systemd, &manifest(None), &both);
    assert_eq!(low.len(), 1, "{low:#?}");
    assert!(
        low[0]
            .message
            .contains("web.https on port 443, web.http on port 80"),
        "{}",
        low[0].message
    );
}

#[test]
fn the_tunnel_sharing_the_web_port_counts_as_one_low_port() {
    let shared = load(
        "listen: 7835\nweb: { https: 443, letsencrypt: { email: ops@example.com } }\nwebsocket: { listen: 443, host: tunnel.example.com, path: /burrow }\n",
    );
    let low = misfits_of(&Systemd, &manifest(Some("/var/lib/burrow/home")), &shared);
    assert_eq!(low.len(), 1, "{low:#?}");
    assert_eq!(
        low[0].message.matches("port 443").count(),
        1,
        "{}",
        low[0].message
    );
}

fn web_config(root: &str) -> burrow_config::ServerConfig {
    let at = |p: &str| format!("{root}{p}");
    serde_json::from_value(serde_json::json!({
        "web": {
            "https": 443,
            "error_pages": at("pages"),
            "access_log": at("logs/access.log"),
            "certificates": [{"hosts": ["a.example.com"], "cert": at("certs/a.pem"), "key": at("certs/a.key")}],
            "letsencrypt": {
                "email": "ops@example.com",
                "ca": at("certs/ca.pem"),
                "dns": [
                    {"domains": ["example.com"], "cloudflare": {"token": format!("file:{}", at("cf.token"))}},
                    {"domains": ["example.org"], "command": at("hooks/dns.sh")},
                    {"domains": ["example.net"], "cloudflare": {"token": "${CF_TOKEN}"}}
                ]
            }
        },
        "services": {"blog": {"client": "home", "target": 80, "web": {
            "hosts": ["blog.example.net"],
            "tls": {"cert": at("certs/b.pem"), "key": at("certs/b.key")},
            "error_pages": at("blog-pages"),
            "target_tls": {"ca": at("certs/app-ca.pem")}
        }}}
    }))
    .unwrap()
}

const WEB_FIELDS: [&str; 10] = [
    "services.blog.web.tls.cert",
    "services.blog.web.tls.key",
    "services.blog.web.target_tls.ca",
    "services.blog.web.error_pages",
    "web.certificates[0].cert",
    "web.certificates[0].key",
    "web.error_pages",
    "web.letsencrypt.ca",
    "web.letsencrypt.dns[0].cloudflare.token",
    "web.letsencrypt.dns[1].command",
];

#[test]
fn every_file_the_web_proxy_reads_is_refused_at_install_when_relative() {
    let cfg = web_config("");
    let files = super::web_files::read(&cfg);
    let fields: Vec<&str> = files.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(fields, WEB_FIELDS);
    assert_eq!(files[8].1, PathBuf::from("cf.token"));
    for (field, path) in &files {
        let refused = super::checks::refuse_relative(&[(field.clone(), path.clone())])
            .expect_err(field)
            .message;
        assert_eq!(
            refused,
            format!(
                "{field} is a relative path ({}); a service needs absolute paths",
                path.display()
            )
        );
    }
    assert_eq!(
        super::web_files::access_log(&cfg),
        Some(PathBuf::from("logs/access.log"))
    );
}

#[test]
fn a_web_file_the_service_account_cannot_read_or_reach_is_reported() {
    let m = manifest(Some("/var/lib/burrow/home"));
    for (field, path) in super::web_files::read(&web_config("/nonexistent-burrow-test/")) {
        assert_eq!(
            Systemd.cannot_use(&m, &path).as_deref(),
            Some("its account burrow-home cannot read it"),
            "{field}"
        );
    }
    let home = super::web_files::read(&web_config("/home/someone/"));
    let (field, path) = &home[6];
    assert_eq!(field, "web.error_pages");
    assert_eq!(
        Systemd.cannot_use(&m, path).as_deref(),
        Some("it is in a home folder, which the service was installed without access to")
    );
}

#[test]
fn the_access_log_must_be_a_file_of_its_own_in_the_services_log_folder() {
    use super::web_files::log_folder_refusal;
    let own = PathBuf::from("/var/log/burrow/home/home.log");
    let refused = |p: &str| log_folder_refusal(&PathBuf::from(p), &own, false);
    assert_eq!(refused("/var/log/burrow/home/access.log"), None);
    assert_eq!(
        refused("/var/log/burrow-web/access.log").unwrap(),
        "web.access_log (/var/log/burrow-web/access.log) must be a file directly in the service's log folder, which only the service can write, and not its own log: use /var/log/burrow/home/access.log"
    );
    for (path, use_) in [
        ("logs/web.log", "/var/log/burrow/home/web.log"),
        (
            "/var/log/burrow/home/../x.log",
            "/var/log/burrow/home/x.log",
        ),
        (
            "/var/log/burrow/home/web/access.log",
            "/var/log/burrow/home/access.log",
        ),
        (
            "/var/log/burrow/home/home.log",
            "/var/log/burrow/home/access.log",
        ),
        (
            "/var/log/burrow/home/home.log.1",
            "/var/log/burrow/home/access.log",
        ),
    ] {
        let why = refused(path).unwrap_or_else(|| panic!("{path} passed"));
        assert!(why.ends_with(&format!("use {use_}")), "{path}: {why}");
    }
    let shared = PathBuf::from("/data/burrow/logs/home.log");
    let win = |p: &str| log_folder_refusal(&PathBuf::from(p), &shared, true);
    assert_eq!(win("/data/burrow/logs/home-access.log"), None);
    for other in [
        "/data/burrow/logs/access.log",
        "/data/burrow/logs/office.log",
    ] {
        assert!(
            win(other)
                .unwrap()
                .ends_with("use /data/burrow/logs/home-access.log"),
            "{other}"
        );
    }
}
