use std::path::Path;
use std::time::Duration;

use super::later::*;
use super::tests::{NAS, errors, load, settings};
use crate::config::settings::{WebSite, WebTls};
use burrow_config::ServerConfig;
use serde_json::json;

fn codes(rest: &str) -> Vec<(&'static str, String)> {
    load(rest)
        .report
        .diagnostics
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| (d.code, d.message))
        .collect()
}

#[test]
fn every_web_key_of_a_later_phase_is_refused_with_its_own_code() {
    let cases = [(
        WEB_HTTPS_REDIRECT,
        "https_redirect: false",
        "web.https_redirect",
    )];
    for (code, yaml, key) in cases {
        let got = codes(&format!(
            "listen: 7835\nweb:\n  https: 443\n  {yaml}\n{NAS}"
        ));
        assert_eq!(
            got,
            [(code, format!("{key}: not in this build yet"))],
            "{yaml}"
        );
    }
}

#[test]
fn a_site_that_sets_its_own_redirect_is_refused_until_it_is_served() {
    let nas = NAS.replace(
        "tls: passthrough }",
        "tls: passthrough, https_redirect: true }",
    );
    let got = codes(&format!("listen: 7835\nweb: {{ https: 443 }}\n{nas}"));
    assert_eq!(
        got,
        [(
            SITE_HTTPS_REDIRECT,
            "services.nas.web.https_redirect: not in this build yet".to_string()
        )]
    );
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
    let blog = super::fill::site(&cfg, web, vec!["blog.example.com".into()], base);
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
