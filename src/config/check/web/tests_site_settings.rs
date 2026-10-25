use std::path::Path;
use std::time::Duration;

use crate::config::settings::{LogFormat, NameRedirect, PathRedirect, TargetTls};
use burrow_config::ServerConfig;
use serde_json::json;

fn config() -> ServerConfig {
    serde_json::from_value(json!({
        "web": {
            "https": 443,
            "redirect_names": {
                "old.example.com": "https://example.com",
                "Example.com/shop/": {"to": "https://shop.example.net/", "permanent": true}
            },
            "trusted_proxies": ["173.245.48.0/20"],
            "proxy_protocol_from": ["10.0.0.5/32"],
            "access_log": {"file": "logs/access.log", "format": "json"},
            "request_id": true,
            "hsts": true,
            "max_body_size": "100MB",
            "response_headers": {"Server": "", "X-Content-Type-Options": "nosniff"},
            "error_pages": "pages"
        },
        "services": {
            "api": {"client": "home", "target": "erp.lan:8443", "web": {
                "hosts": ["example.com"],
                "paths": ["/%61pi/", "/v2"],
                "app_path": "/v1",
                "redirect_from": ["WWW.example.com"],
                "redirects": {"/old": "/new", "/feed": {"to": "https://feeds.example.net", "permanent": true}},
                "maintenance": true,
                "target_tls": {"ca": "certs/corp-ca.pem"},
                "hsts": false,
                "max_body_size": "8GB",
                "response_header_timeout_secs": 300,
                "response_headers": {"server": "burrow", "Access-Control-Allow-Origin": "https://example.com"}
            }},
            "blog": {"client": "home", "target": 8080, "web": "blog.example.com"}
        }
    }))
    .unwrap()
}

#[test]
fn a_sites_keys_reach_its_settings_on_top_of_the_web_defaults() {
    let cfg = config();
    let base = Path::new("/etc/burrow");
    let site = |name: &str| {
        let svc = &cfg.services[name];
        super::fill::site(&cfg, svc.web.as_ref().unwrap(), &svc.target, vec![], base).options
    };
    let api = site("api");
    assert_eq!(api.paths, ["/api", "/v2"]);
    assert_eq!(api.app_path.as_deref(), Some("/v1"));
    assert_eq!(api.redirect_from, ["www.example.com"]);
    assert_eq!(
        api.redirects,
        [
            PathRedirect {
                from: "/feed".into(),
                to: "https://feeds.example.net".into(),
                permanent: true
            },
            PathRedirect {
                from: "/old".into(),
                to: "/new".into(),
                permanent: false
            },
        ]
    );
    assert!(api.maintenance);
    assert_eq!(
        api.target_tls,
        TargetTls::Ca(base.join("certs/corp-ca.pem"))
    );
    assert_eq!(api.target_name, "erp.lan");
    assert!(!api.hsts, "the site's own false wins over web.hsts");
    assert_eq!(api.max_body_size, Some(8_000_000_000));
    assert_eq!(api.response_header_timeout, Duration::from_secs(300));
    assert_eq!(
        api.response_headers,
        [
            (
                "Access-Control-Allow-Origin".to_string(),
                "https://example.com".to_string()
            ),
            ("server".to_string(), "burrow".to_string()),
            ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
        ]
    );
    assert_eq!(api.error_pages, Some(base.join("pages")));
    let blog = site("blog");
    assert!(blog.paths.is_empty() && blog.redirects.is_empty());
    assert!(blog.hsts, "the short form takes every web default");
    assert_eq!(blog.max_body_size, Some(100_000_000));
    assert_eq!(blog.response_header_timeout, Duration::from_secs(60));
    assert_eq!(blog.target_tls, TargetTls::Off);
    assert_eq!(blog.target_name, "");
    assert_eq!(blog.response_headers.len(), 2);
}

#[test]
fn the_web_wide_keys_reach_the_settings() {
    let cfg = config();
    let base = Path::new("/etc/burrow");
    let s = super::fill::settings(
        &cfg,
        (Some("[::]:443".parse().unwrap()), None),
        Default::default(),
        None,
        base,
    )
    .unwrap();
    assert_eq!(
        s.redirect_names,
        [
            NameRedirect {
                host: "example.com".into(),
                path: "/shop".into(),
                to: "https://shop.example.net/".into(),
                permanent: true
            },
            NameRedirect {
                host: "old.example.com".into(),
                path: "/".into(),
                to: "https://example.com".into(),
                permanent: false
            },
        ]
    );
    assert_eq!(s.trusted_proxies, ["173.245.48.0/20".parse().unwrap()]);
    assert_eq!(s.proxy_protocol_from, ["10.0.0.5/32".parse().unwrap()]);
    assert_eq!(
        s.access_log,
        Some((base.join("logs/access.log"), LogFormat::Json))
    );
    assert!(s.request_id);
}

#[test]
fn web_https_redirect_is_the_default_of_https_sites_only() {
    let cfg: ServerConfig = serde_json::from_value(json!({
        "web": {"https": 443, "http": 80, "https_redirect": true},
        "services": {
            "lan": {"client": "home", "target": 8095, "web": {"hosts": ["dash.lan.example.com"], "tls": "off"}},
            "cdn": {"client": "home", "target": 3001, "web": {"hosts": ["wiki.example.net"], "tls": "off", "https_redirect": true}},
            "shop": {"client": "home", "target": 8081, "web": "shop.example.com"}
        }
    }))
    .unwrap();
    let redirect = |name: &str| {
        let svc = &cfg.services[name];
        super::fill::site(
            &cfg,
            svc.web.as_ref().unwrap(),
            &svc.target,
            vec![],
            Path::new("/"),
        )
        .https_redirect
    };
    assert!(
        !redirect("lan"),
        "a plain site does not follow web.https_redirect"
    );
    assert!(
        redirect("cdn"),
        "its own https_redirect: true behind a proxy"
    );
    assert!(redirect("shop"));
}
