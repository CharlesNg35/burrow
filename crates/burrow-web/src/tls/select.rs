use std::collections::HashSet;
use std::path::PathBuf;

use burrow_syntax::web::{Twice, entry_for, is_wildcard, shared_wildcard_covers, sni_name};

use super::{CertName, CertOrigin};
use crate::{CertSettings, CertSource, ProxySettings, SiteTls, TunnelMode, TunnelPort};

pub const TUNNEL_GROUP: &str = "websocket.host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    File {
        cert: PathBuf,
        key: PathBuf,
        origin: CertOrigin,
    },
    Acme,
    Internal,
    Missing(String),
}

fn listed_wildcards(settings: &ProxySettings) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in settings.certs.acme.iter().flat_map(|a| a.dns.iter()) {
        for w in &e.wildcards {
            let w = sni_name(w.trim());
            if is_wildcard(&w) && !out.contains(&w) {
                out.push(w);
            }
        }
    }
    out
}

pub fn cert_names(settings: &ProxySettings) -> Vec<CertName> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |name: &str, source: CertSource, group: &str| {
        if seen.insert(name.to_ascii_lowercase()) {
            out.push(CertName {
                name: name.to_ascii_lowercase(),
                source,
                group: group.to_string(),
            });
        }
    };
    let wildcards = listed_wildcards(settings);
    let shared = |host: &str| {
        let entries = settings
            .certs
            .shared
            .iter()
            .enumerate()
            .map(|(i, f)| (i, f.hosts.as_slice()));
        entry_for(entries, host) != Ok(None)
    };
    let shares = |host: &str| {
        let h = sni_name(host);
        !shared(&h) && wildcards.iter().any(|w| shared_wildcard_covers(w, &h))
    };
    for site in &settings.sites {
        if let SiteTls::Https(source) = &site.tls {
            for host in site.hosts.iter().chain(&site.options.redirect_from) {
                if !(*source == CertSource::Auto && shares(host)) {
                    push(host, source.clone(), &site.service);
                }
            }
        }
    }
    if settings.https.is_some() {
        for r in settings.redirect_names.iter().filter(|r| !shares(&r.host)) {
            push(
                &r.host,
                CertSource::Auto,
                &format!("web.redirect_names {}", r.host),
            );
        }
    }
    for w in &wildcards {
        push(w, CertSource::Auto, w);
        push(w.trim_start_matches("*."), CertSource::Auto, w);
    }
    if let Some(t) = settings
        .tunnel
        .as_ref()
        .filter(|t| t.mode == TunnelMode::OwnName && t.port == TunnelPort::Https)
    {
        let source = settings
            .certs
            .tunnel_file
            .as_ref()
            .map_or(CertSource::Auto, |(cert, key)| CertSource::OwnFile {
                cert: cert.clone(),
                key: key.clone(),
            });
        if !(source == CertSource::Auto && shares(&t.host)) {
            push(&t.host, source, TUNNEL_GROUP);
        }
    }
    out
}

pub fn wildcard_checks(settings: &CertSettings, names: &[CertName]) -> CertSettings {
    let mut out = settings.clone();
    for n in names.iter().filter(|n| is_wildcard(&n.group)) {
        out.choices
            .entry(n.group.clone())
            .or_insert(burrow_syntax::web::Challenge::Dns);
    }
    out
}

pub fn choose(settings: &CertSettings, name: &CertName) -> Choice {
    match &name.source {
        CertSource::OwnFile { cert, key } => Choice::File {
            cert: cert.clone(),
            key: key.clone(),
            origin: if name.group == TUNNEL_GROUP {
                CertOrigin::TunnelFile { path: cert.clone() }
            } else {
                CertOrigin::OwnFile { path: cert.clone() }
            },
        },
        CertSource::Internal => Choice::Internal,
        CertSource::Auto => {
            let entries = settings
                .shared
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.hosts.as_slice()));
            match (entry_for(entries, &name.name), &settings.acme) {
                (Ok(Some(i)), _) => {
                    let f = &settings.shared[i];
                    Choice::File {
                        cert: f.cert.clone(),
                        key: f.key.clone(),
                        origin: CertOrigin::SharedFile {
                            path: f.cert.clone(),
                        },
                    }
                }
                (Err(Twice { first, second }), _) => Choice::Missing(format!(
                    "\"{}\" is listed in web.certificates[{first}] and [{second}]",
                    name.name
                )),
                (Ok(None), Some(_)) => Choice::Acme,
                (Ok(None), None) => {
                    Choice::Missing(format!("\"{}\" has no certificate", name.name))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcmeSettings, SharedFile, Site, TunnelSettings};

    fn own(cert: &str) -> CertSource {
        CertSource::OwnFile {
            cert: cert.into(),
            key: format!("{cert}.key").into(),
        }
    }

    fn name(n: &str, source: CertSource) -> CertName {
        CertName {
            name: n.into(),
            source,
            group: "blog".into(),
        }
    }

    fn certs(acme: bool) -> CertSettings {
        CertSettings {
            shared: vec![SharedFile {
                hosts: vec!["*.example.com".into(), "Shared.Example.org".into()],
                cert: "star.pem".into(),
                key: "star.key".into(),
            }],
            tunnel_file: None,
            acme: acme.then(|| AcmeSettings {
                directory_url: "https://acme.test/dir".into(),
                ca: None,
                email: "ops@example.com".into(),
                profile: None,
                state_dir: None,
                check_ports: Default::default(),
                dns: Vec::new(),
                fallback_directory: None,
            }),
            ..CertSettings::default()
        }
    }

    fn origin(c: Choice) -> Option<CertOrigin> {
        match c {
            Choice::File { origin, .. } => Some(origin),
            _ => None,
        }
    }

    #[test]
    fn the_own_file_wins_then_a_shared_file_listing_the_name_then_acme() {
        let s = certs(true);
        let path = |p: &str| PathBuf::from(p);
        assert_eq!(
            origin(choose(&s, &name("*.example.com", own("own.pem")))),
            Some(CertOrigin::OwnFile {
                path: path("own.pem")
            })
        );
        assert_eq!(
            origin(choose(&s, &name("*.example.com", CertSource::Auto))),
            Some(CertOrigin::SharedFile {
                path: path("star.pem")
            })
        );
        assert_eq!(
            origin(choose(&s, &name("shared.example.org", CertSource::Auto))),
            Some(CertOrigin::SharedFile {
                path: path("star.pem")
            })
        );
        assert_eq!(
            origin(choose(&s, &name("x.example.com", CertSource::Auto))),
            Some(CertOrigin::SharedFile {
                path: path("star.pem")
            }),
            "a listed wildcard covers one label below it"
        );
        for not in ["a.b.example.com", "example.com"] {
            assert_eq!(
                choose(&s, &name(not, CertSource::Auto)),
                Choice::Acme,
                "{not}"
            );
        }
        assert!(matches!(
            choose(&certs(false), &name("a.b.example.com", CertSource::Auto)),
            Choice::Missing(_)
        ));
    }

    #[test]
    fn the_tunnel_by_path_on_a_sites_name_shares_that_sites_certificate() {
        let mut p = ProxySettings::new("[::]:443".parse().unwrap());
        p.sites = vec![Site {
            tls: SiteTls::Https(CertSource::Auto),
            ..Site::passthrough("blog", vec!["blog.example.com".into()])
        }];
        p.tunnel = Some(TunnelSettings {
            host: "blog.example.com".into(),
            path: "/burrow".into(),
            mode: TunnelMode::ByPath,
            port: crate::TunnelPort::Https,
        });
        p.certs.tunnel_file = Some(("ws.pem".into(), "ws.key".into()));
        let names = cert_names(&p);
        assert_eq!(names.len(), 1, "one name, so one order: {names:#?}");
        assert_eq!(names[0].group, "blog");
        assert_eq!(names[0].source, CertSource::Auto);
    }

    #[test]
    fn an_exact_listing_beats_a_wildcard_and_a_name_listed_twice_gets_no_file() {
        let mut s = certs(true);
        s.shared.push(SharedFile {
            hosts: vec!["app.example.com".into()],
            cert: "app.pem".into(),
            key: "app.key".into(),
        });
        assert_eq!(
            origin(choose(&s, &name("app.example.com", CertSource::Auto))),
            Some(CertOrigin::SharedFile {
                path: "app.pem".into()
            })
        );
        s.shared.push(SharedFile {
            hosts: vec!["App.Example.com".into()],
            cert: "again.pem".into(),
            key: "again.key".into(),
        });
        assert_eq!(
            choose(&s, &name("app.example.com", CertSource::Auto)),
            Choice::Missing("\"app.example.com\" is listed in web.certificates[1] and [2]".into())
        );
    }

    #[test]
    fn the_tunnel_name_uses_websocket_tls_then_the_same_order_as_a_site() {
        let mut p = ProxySettings::new("[::]:443".parse().unwrap());
        p.sites = vec![
            Site {
                tls: SiteTls::Https(CertSource::Auto),
                ..Site::passthrough(
                    "blog",
                    vec!["Blog.example.com".into(), "www.example.com".into()],
                )
            },
            Site {
                tls: SiteTls::Https(own("shop.pem")),
                ..Site::passthrough(
                    "shop",
                    vec!["www.example.com".into(), "shop.example.com".into()],
                )
            },
            Site::passthrough("nas", vec!["nas.example.com".into()]),
            Site {
                tls: SiteTls::Plain,
                ..Site::passthrough("wiki", vec!["wiki".into()])
            },
        ];
        p.tunnel = Some(TunnelSettings {
            host: "tunnel.example.com".into(),
            path: "/burrow".into(),
            mode: TunnelMode::OwnName,
            port: crate::TunnelPort::Https,
        });
        p.certs.tunnel_file = Some(("ws.pem".into(), "ws.key".into()));
        let names = cert_names(&p);
        let got: Vec<(&str, &str)> = names
            .iter()
            .map(|n| (n.name.as_str(), n.group.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("blog.example.com", "blog"),
                ("www.example.com", "blog"),
                ("shop.example.com", "shop"),
                ("tunnel.example.com", TUNNEL_GROUP),
            ]
        );
        assert_eq!(
            origin(choose(&p.certs, &names[3])),
            Some(CertOrigin::TunnelFile {
                path: "ws.pem".into()
            })
        );
        p.certs.tunnel_file = None;
        assert_eq!(cert_names(&p)[3].source, CertSource::Auto);
        p.tunnel.as_mut().unwrap().mode = TunnelMode::ByPath;
        assert_eq!(cert_names(&p).len(), 3);
    }

    #[test]
    fn a_tunnel_on_the_plain_http_port_needs_no_certificate() {
        let mut p = ProxySettings::new("[::]:443".parse().unwrap());
        p.tunnel = Some(TunnelSettings {
            host: "tunnel.example.com".into(),
            path: "/burrow".into(),
            mode: TunnelMode::OwnName,
            port: crate::TunnelPort::Http,
        });
        assert!(cert_names(&p).is_empty());
        p.tunnel.as_mut().unwrap().port = crate::TunnelPort::Https;
        assert_eq!(cert_names(&p).len(), 1);
    }

    #[test]
    fn sites_one_label_under_a_listed_wildcard_share_its_one_certificate() {
        let mut p = ProxySettings::new("[::]:443".parse().unwrap());
        let mut c = certs(true);
        c.shared.clear();
        c.shared.push(SharedFile {
            hosts: vec!["shop.example.com".into()],
            cert: "shop.pem".into(),
            key: "shop.key".into(),
        });
        if let Some(a) = c.acme.as_mut() {
            a.dns = vec![crate::DnsEntry {
                domains: vec!["example.com".into()],
                provider: crate::DnsProvider::Command {
                    path: "/usr/local/bin/burrow-dns".into(),
                },
                wait: crate::tls::acme::dns01::DNS_WAIT,
                resolvers: Vec::new(),
                wildcards: vec!["*.Example.com".into()],
            }];
        }
        p.certs = c;
        let https = |svc: &str, hosts: &[&str], tls: CertSource| Site {
            tls: SiteTls::Https(tls),
            ..Site::passthrough(svc, hosts.iter().map(|h| h.to_string()).collect())
        };
        p.sites = vec![
            https(
                "wiki",
                &["wiki.example.com", "example.com"],
                CertSource::Auto,
            ),
            https("all", &["*.example.com"], CertSource::Auto),
            https("deep", &["a.b.example.com"], CertSource::Auto),
            https("shop", &["shop.example.com"], CertSource::Auto),
            https("own", &["own.example.com"], own("own.pem")),
        ];
        let names = cert_names(&p);
        assert_eq!(
            wildcard_checks(&p.certs, &names)
                .choices
                .get("*.example.com"),
            Some(&burrow_syntax::web::Challenge::Dns),
            "the parent name of a shared wildcard is checked over DNS too"
        );
        let got: Vec<(String, String)> = names.into_iter().map(|n| (n.name, n.group)).collect();
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            got,
            vec![
                pair("a.b.example.com", "deep"),
                pair("shop.example.com", "shop"),
                pair("own.example.com", "own"),
                pair("*.example.com", "*.example.com"),
                pair("example.com", "*.example.com"),
            ]
        );
    }

    #[test]
    fn names_that_only_redirect_get_certificates_too() {
        let mut p = ProxySettings::new("[::]:443".parse().unwrap());
        let mut blog = Site {
            tls: SiteTls::Https(CertSource::Auto),
            ..Site::passthrough("blog", vec!["blog.example.com".into()])
        };
        blog.options.redirect_from = vec!["www.blog.example.com".into()];
        p.sites = vec![blog];
        p.redirect_names = vec![crate::NameRedirect {
            host: "old.example.com".into(),
            path: "/".into(),
            to: "https://blog.example.com".into(),
            permanent: false,
        }];
        let got: Vec<(String, String)> = cert_names(&p)
            .into_iter()
            .map(|n| (n.name, n.group))
            .collect();
        assert_eq!(
            got,
            [
                ("blog.example.com".into(), "blog".into()),
                ("www.blog.example.com".into(), "blog".into()),
                (
                    "old.example.com".into(),
                    "web.redirect_names old.example.com".into()
                ),
            ]
        );
        p.https = None;
        assert_eq!(cert_names(&p).len(), 2);
    }
}
