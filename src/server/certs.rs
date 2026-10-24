use burrow_admin_proto::status::{
    AcmeStatus, CertCheck, CertSourceStatus, CertStateStatus, CertStatus,
};
use burrow_web::tls::{self, CertOrigin, CertPhase, Certificates};

use super::Shared;
use crate::config::settings::{ServerSettings, WebTls};

impl Shared {
    pub(super) fn certificate_lines(
        &self,
        settings: &ServerSettings,
    ) -> (Vec<CertStatus>, Option<AcmeStatus>) {
        let Some(web) = &settings.web else {
            return (Vec::new(), None);
        };
        let mut lines: Vec<CertStatus> = self.certs.status().into_iter().map(line).collect();
        for site in web.sites.values() {
            let source = match site.tls {
                WebTls::Passthrough => CertSourceStatus::Passthrough,
                WebTls::Off => CertSourceStatus::Plain,
                _ => continue,
            };
            for host in &site.hosts {
                lines.push(CertStatus {
                    name: host.clone(),
                    source: source.clone(),
                    check: None,
                    state: CertStateStatus::Ok,
                    expires: None,
                    next_renewal: None,
                    last_error: None,
                    explanation: None,
                    failures: 0,
                });
            }
        }
        let acme = web.letsencrypt.as_ref().map(|l| AcmeStatus {
            directory: l.directory.clone(),
            account: self.certs.account(),
            state_dir: l
                .state_dir
                .as_ref()
                .map(|d| d.display().to_string())
                .unwrap_or_default(),
        });
        (lines, acme)
    }
}

fn line(s: tls::CertStatus) -> CertStatus {
    let path = |p: std::path::PathBuf| p.display().to_string();
    CertStatus {
        name: s.name,
        source: match s.origin {
            CertOrigin::OwnFile { path: p } => CertSourceStatus::OwnFile { path: path(p) },
            CertOrigin::SharedFile { path: p } => CertSourceStatus::SharedFile { path: path(p) },
            CertOrigin::TunnelFile { path: p } => CertSourceStatus::TunnelFile { path: path(p) },
            CertOrigin::LetsEncrypt { directory } => CertSourceStatus::LetsEncrypt { directory },
        },
        check: s.check.map(|c| match c {
            tls::CertCheck::Tls => CertCheck::Tls,
            tls::CertCheck::Http => CertCheck::Http,
        }),
        state: match s.phase {
            CertPhase::Ok => CertStateStatus::Ok,
            CertPhase::Pending => CertStateStatus::Pending,
            CertPhase::Renewing => CertStateStatus::Renewing,
            CertPhase::Failing { reason } => CertStateStatus::Failing { reason },
        },
        expires: s.expires,
        next_renewal: s.next_renewal,
        last_error: s.last_error,
        explanation: s.explanation,
        failures: s.failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_part_of_a_certificate_line_maps_to_the_status_snapshot() {
        let got = line(tls::CertStatus {
            name: "blog.example.com".into(),
            origin: CertOrigin::LetsEncrypt {
                directory: "https://acme.test/dir".into(),
            },
            check: Some(tls::CertCheck::Http),
            phase: CertPhase::Failing {
                reason: "rate limited".into(),
            },
            expires: Some(10),
            next_renewal: Some(20),
            last_error: Some("rate limited".into()),
            explanation: Some("https://example.com/why".into()),
            failures: 2,
        });
        assert_eq!(
            got.source,
            CertSourceStatus::LetsEncrypt {
                directory: "https://acme.test/dir".into()
            }
        );
        assert_eq!(got.check, Some(CertCheck::Http));
        assert_eq!(
            got.state,
            CertStateStatus::Failing {
                reason: "rate limited".into()
            }
        );
        assert_eq!(
            (got.expires, got.next_renewal, got.failures),
            (Some(10), Some(20), 2)
        );
        assert_eq!(got.explanation.as_deref(), Some("https://example.com/why"));
        let own = line(tls::CertStatus {
            origin: CertOrigin::TunnelFile {
                path: "/etc/burrow/ws.pem".into(),
            },
            phase: CertPhase::Ok,
            check: None,
            ..tls_status()
        });
        assert_eq!(
            own.source,
            CertSourceStatus::TunnelFile {
                path: "/etc/burrow/ws.pem".into()
            }
        );
        assert_eq!(own.state, CertStateStatus::Ok);
    }

    fn tls_status() -> tls::CertStatus {
        tls::CertStatus {
            name: "x".into(),
            origin: CertOrigin::OwnFile { path: "a".into() },
            check: None,
            phase: CertPhase::Pending,
            expires: None,
            next_renewal: None,
            last_error: None,
            explanation: None,
            failures: 0,
        }
    }
}
