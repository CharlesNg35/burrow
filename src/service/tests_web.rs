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
fn lets_encrypt_without_the_folder_is_named_for_a_reinstall() {
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
