use std::path::Path;
use std::time::Duration;

use super::tests::{NAS, errors, load, settings};
use crate::config::settings::{WebSite, WebTls};
use burrow_config::ServerConfig;
use serde_json::json;

#[test]
fn https_redirect_is_taken_from_the_site_then_from_web() {
    let blog = |site: &str, top: &str| {
        format!(
            "listen: 7835\nweb: {{ https: 443, http: 80{top} }}\nservices:\n  blog:\n    client: home\n    target: 8080\n    web: {{ hosts: [blog.example.com], tls: {{ cert: c.pem, key: k.pem }}{site} }}\n"
        )
    };
    for (site, top, want) in [
        ("", "", true),
        ("", ", https_redirect: false", false),
        (", https_redirect: true", ", https_redirect: false", true),
        (", https_redirect: false", "", false),
    ] {
        let o = load(&blog(site, top));
        let found: Vec<String> = errors(&o)
            .into_iter()
            .filter(|e| e.contains("https_redirect"))
            .collect();
        assert!(found.is_empty(), "{site}{top}: {found:?}");
        let web = settings(o).web.unwrap();
        assert_eq!(
            web.site_of("blog").unwrap().https_redirect,
            want,
            "{site}{top}"
        );
    }
}

#[test]
fn a_passthrough_site_keeps_the_defaults_of_every_new_setting() {
    let web = settings(load(&format!("listen: 7835\nweb: {{ https: 443 }}\n{NAS}")))
        .web
        .unwrap();
    assert_eq!(
        web.site_of("nas"),
        Some(&WebSite::passthrough(vec!["nas.example.com".into()]))
    );
    assert_eq!(web.http, None);
    assert_eq!(web.request_header_timeout, Duration::from_secs(10));
    assert_eq!(web.idle_timeout, Duration::from_secs(60));
    assert!(web.certificates.is_empty() && web.letsencrypt.is_none());
    assert!(
        errors(&load(&format!(
            "listen: 7835\nweb: {{ https: 443 }}\n{NAS}"
        )))
        .is_empty()
    );
}

#[test]
fn the_new_keys_are_wired_into_the_settings_once_their_refusal_is_lifted() {
    let cfg: ServerConfig = serde_json::from_value(json!({
        "web": {
            "https": 443,
            "https_redirect": false,
            "letsencrypt": {"email": "ops@example.com", "directory": "staging", "ca": "ca.pem"},
            "certificates": [{"hosts": ["*.example.com"], "cert": "w.pem", "key": "w.key"}],
            "request_header_timeout_secs": 5,
            "idle_timeout_secs": 30
        },
        "services": {"blog": {"client": "home", "target": 80, "web": {
            "hosts": ["blog.example.com"], "tls": {"cert": "b.pem", "key": "b.key"}
        }}}
    }))
    .unwrap();
    let base = Path::new("/etc/burrow");
    let web = cfg.services["blog"].web.as_ref().unwrap();
    let blog = super::fill::site(
        &cfg,
        web,
        &cfg.services["blog"].target,
        vec!["blog.example.com".into()],
        base,
    );
    assert_eq!(
        blog.tls,
        WebTls::Files {
            cert: base.join("b.pem"),
            key: base.join("b.key")
        }
    );
    assert!(
        !blog.https_redirect,
        "web.https_redirect is the site default"
    );
    let s = super::fill::settings(
        &cfg,
        (Some("[::]:443".parse().unwrap()), None),
        Default::default(),
        None,
        base,
    )
    .unwrap();
    assert_eq!(s.request_header_timeout, Duration::from_secs(5));
    assert_eq!(s.idle_timeout, Duration::from_secs(30));
    assert_eq!(s.certificates[0].cert, base.join("w.pem"));
    let le = s.letsencrypt.unwrap();
    assert_eq!(
        le.directory,
        "https://acme-staging-v02.api.letsencrypt.org/directory"
    );
    assert_eq!(le.ca, Some(base.join("ca.pem")));
}

#[test]
fn the_header_and_idle_timeouts_are_taken_from_the_config() {
    let o = load(&format!(
        "listen: 7835\nweb: {{ https: 443, request_header_timeout_secs: 5, idle_timeout_secs: 30 }}\n{NAS}"
    ));
    assert!(errors(&o).is_empty(), "{:?}", errors(&o));
    let web = settings(o).web.unwrap();
    assert_eq!(web.request_header_timeout, Duration::from_secs(5));
    assert_eq!(web.idle_timeout, Duration::from_secs(30));
}

fn letsencrypt(dir: &Path, fields: &str) -> Vec<(String, Option<String>)> {
    super::tests::load_in(
        dir,
        &format!(
            "listen: 7835\nweb:\n  https: 443\n  letsencrypt: {{ {fields} }}\nservices:\n  blog: {{ client: home, target: 80, web: blog.example.com }}\n"
        ),
    )
    .report
    .diagnostics
    .into_iter()
    .filter(|d| d.is_error())
    .map(|d| (d.message, d.help))
    .collect()
}

#[test]
fn the_letsencrypt_email_directory_and_ca_are_checked_at_validate() {
    let dir = tempfile::tempdir().unwrap();
    let only = |fields: &str| -> Vec<String> {
        letsencrypt(dir.path(), fields)
            .into_iter()
            .map(|e| e.0)
            .collect()
    };
    for fine in [
        "email: ops@example.com",
        "email: ops@example.com, directory: staging",
        "email: ops@example.com, directory: https://ca.internal/acme/directory",
    ] {
        assert_eq!(only(fine), Vec::<String>::new(), "{fine}");
    }
    assert_eq!(
        only("email: ops"),
        ["web.letsencrypt.email \"ops\" is not an email address"]
    );
    let typo = letsencrypt(dir.path(), "email: ops@example.com, directory: stagging");
    assert_eq!(
        typo[0].0,
        "web.letsencrypt.directory \"stagging\" is not production, staging or an https:// URL"
    );
    assert!(
        typo[0]
            .1
            .as_deref()
            .unwrap()
            .starts_with("did you mean \"staging\"?"),
        "{typo:?}"
    );
    for url in ["https//x", "http://ca.internal/acme/directory"] {
        assert_eq!(
            only(&format!("email: ops@example.com, directory: \"{url}\"")),
            [format!(
                "web.letsencrypt.directory \"{url}\" is not production, staging or an https:// URL"
            )]
        );
    }
    let ca = "email: ops@example.com, ca: certs/ca.pem";
    assert_eq!(
        only(ca),
        ["web.letsencrypt.ca: cannot read certs/ca.pem: file not found"]
    );
    std::fs::create_dir_all(dir.path().join("certs")).unwrap();
    std::fs::write(dir.path().join("certs/ca.pem"), "not a certificate\n").unwrap();
    assert_eq!(
        only(ca),
        ["web.letsencrypt.ca: certs/ca.pem holds no certificate"]
    );
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let cert = params.self_signed(&key).unwrap();
    std::fs::write(dir.path().join("certs/ca.pem"), cert.pem()).unwrap();
    assert_eq!(only(ca), Vec::<String>::new());
}
