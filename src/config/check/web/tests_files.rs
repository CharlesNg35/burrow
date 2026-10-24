use std::path::Path;

use crate::config::settings::{WebSettings, WebTls};
use burrow_config::ServerConfig;
use serde_json::json;

fn pair(dir: &Path, stem: &str) {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["blog.example.com".to_string()]).unwrap();
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.join(format!("{stem}.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{stem}.key")), key.serialize_pem()).unwrap();
}

fn web_in(dir: &Path) -> WebSettings {
    let cfg: ServerConfig = serde_json::from_value(json!({
        "web": {
            "https": 443,
            "letsencrypt": {"email": "ops@example.com", "ca": "ca.pem"},
            "certificates": [{"hosts": ["*.example.com"], "cert": "w.pem", "key": "w.key"}]
        },
        "services": {"blog": {"client": "home", "target": 80, "web": {
            "hosts": ["blog.example.com"], "tls": {"cert": "b.pem", "key": "b.key"}
        }}}
    }))
    .unwrap();
    let web = cfg.services["blog"].web.as_ref().unwrap();
    let blog = super::fill::site(&cfg, web, vec!["blog.example.com".into()], dir);
    let sites = [("blog".to_string(), blog)].into_iter().collect();
    super::fill::settings(
        &cfg,
        (Some("[::]:443".parse().unwrap()), None),
        sites,
        None,
        dir,
    )
    .unwrap()
}

#[test]
fn every_certificate_file_is_listed_for_the_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let web = web_in(dir.path());
    assert!(matches!(web.sites["blog"].tls, WebTls::Files { .. }));
    let listed = web.cert_files();
    for f in ["b.pem", "b.key", "w.pem", "w.key", "ca.pem"] {
        assert!(listed.contains(&dir.path().join(f)), "{f}: {listed:?}");
    }
}

#[test]
fn a_certificate_rewritten_in_place_makes_the_settings_differ() {
    let dir = tempfile::tempdir().unwrap();
    for stem in ["b", "w"] {
        pair(dir.path(), stem);
    }
    let first = web_in(dir.path());
    assert_eq!(first, web_in(dir.path()));
    pair(dir.path(), "b");
    let second = web_in(dir.path());
    assert_ne!(first, second, "same paths, new contents");
    std::fs::remove_file(dir.path().join("w.key")).unwrap();
    assert_ne!(
        second,
        web_in(dir.path()),
        "a file that went away counts too"
    );
}

#[test]
fn a_server_with_letsencrypt_loads_with_its_state_folder_and_one_without_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let text = |le: &str| {
        format!(
            "clients:\n  home: {{ secret: \"{}\" }}\nlisten: 7835\nweb:\n  https: 443\n{le}services:\n  nas: {{ client: home, target: 443, web: {{ hosts: [nas.example.com], tls: passthrough }} }}\n",
            super::tests::SECRET
        )
    };
    std::fs::write(&path, text("  letsencrypt: { email: ops@example.com }\n")).unwrap();
    let env = crate::config::env::Env::default();
    let o = crate::config::load_file(&path, Some(crate::config::Role::Server), &env).unwrap();
    let errors: Vec<_> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let Some(crate::config::Loaded::Server(s, _)) = o.loaded else {
        panic!("a server config");
    };
    let le = s.web.unwrap().letsencrypt.unwrap();
    assert_eq!(le.state_dir, Some(crate::install::state_dir_for(&path)));
    assert!(
        !le.state_dir.unwrap().exists(),
        "nothing is made before it is needed"
    );
    std::fs::write(&path, text("")).unwrap();
    let o = crate::config::load_file(&path, Some(crate::config::Role::Server), &env).unwrap();
    let Some(crate::config::Loaded::Server(s, _)) = o.loaded else {
        panic!("a server config");
    };
    assert!(s.web.unwrap().letsencrypt.is_none());
}

#[test]
fn a_sites_and_the_tunnels_chosen_check_reach_the_settings() {
    let cfg: ServerConfig = serde_json::from_value(json!({
        "web": {"https": 443, "http": 80, "letsencrypt": {"email": "ops@example.com"}},
        "websocket": {"listen": 443, "host": "tunnel.example.com", "challenge": "tls"},
        "services": {"blog": {"client": "home", "target": 80, "web": {
            "hosts": ["blog.example.com"], "challenge": "http"
        }}}
    }))
    .unwrap();
    let dir = Path::new("/etc/burrow");
    let web = cfg.services["blog"].web.as_ref().unwrap();
    let blog = super::fill::site(&cfg, web, vec!["blog.example.com".into()], dir);
    assert_eq!(blog.challenge, Some(burrow_syntax::web::Challenge::Http));
    let s = super::fill::settings(
        &cfg,
        (Some("[::]:443".parse().unwrap()), None),
        Default::default(),
        None,
        dir,
    )
    .unwrap();
    assert_eq!(s.tunnel_challenge, Some(burrow_syntax::web::Challenge::Tls));
}
