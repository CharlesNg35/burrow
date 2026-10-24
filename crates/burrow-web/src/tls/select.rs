use std::collections::HashSet;
use std::path::PathBuf;

use burrow_syntax::web::{Twice, entry_for};

use super::{CertName, CertOrigin};
use crate::{CertSettings, CertSource, ProxySettings, SiteTls, TunnelMode};

pub const TUNNEL_GROUP: &str = "websocket.host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    File {
        cert: PathBuf,
        key: PathBuf,
        origin: CertOrigin,
    },
    Acme,
    Missing(String),
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
    for site in &settings.sites {
        if let SiteTls::Https(source) = &site.tls {
            for host in &site.hosts {
                push(host, source.clone(), &site.service);
            }
        }
    }
    if let Some(t) = settings
        .tunnel
        .as_ref()
        .filter(|t| t.mode == TunnelMode::OwnName)
    {
        let source = settings
            .certs
            .tunnel_file
            .as_ref()
            .map_or(CertSource::Auto, |(cert, key)| CertSource::OwnFile {
                cert: cert.clone(),
                key: key.clone(),
            });
        push(&t.host, source, TUNNEL_GROUP);
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
                challenges: Vec::new(),
                fallback_directory: None,
            }),
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
}
