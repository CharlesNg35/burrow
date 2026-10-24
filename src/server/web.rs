use std::net::SocketAddr;
use std::sync::Arc;

use burrow_web::{
    AcmeSettings, BoxIo, CertSettings, CertSource, ProxySettings, Refused, SharedFile, Site,
    SiteTls, TunnelConn, TunnelMode, TunnelPort, TunnelSettings, Upstream, Visitor,
};
use tokio::net::TcpStream;
use tracing::debug;

use super::Shared;
use super::listen::Permit;
use super::route::Outcome;
use super::tcp::{Door, Entry, Refusal, admit, visit};
use crate::base::failure::Failure;
use crate::config::settings::{ServerSettings, Shares, WebSettings, WebTls};
use crate::forward::ForwardError;
use crate::wire::protocol::Status;

pub(super) struct Doorway(Arc<Shared>);

impl Upstream for Doorway {
    type Permit = Permit;

    fn tune(&self, stream: &TcpStream) {
        crate::wire::transport::tcp::tune(stream, &self.0.settings().tcp);
    }

    fn refused(&self) {
        self.0
            .rejected
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.0
            .metrics
            .handshakes_refused
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        let shared = &self.0;
        let settings = shared.settings();
        let svc = settings
            .services
            .get(service)
            .cloned()
            .ok_or(Refused::Gone)?;
        shared
            .still_on_web(service, &from.host)
            .map_err(|_| Refused::Gone)?;
        if !shared.source_allowed(service, from.addr.ip()) {
            shared.metrics.refuse(service);
            debug!(peer = %from.addr, "service \"{service}\": source address not allowed, closing");
            return Err(Refused::NotAllowed);
        }
        let admitted = admit(shared, service, &svc, from.addr, 0).map_err(|r| match r {
            Refusal::Full => Refused::Full,
            Refusal::NoClient => Refused::NoClient,
        })?;
        let (ours, theirs) = burrow_web::pipe();
        theirs.set_reset(true);
        let entry = Entry {
            peer: from.addr,
            local: Some(from.local),
            index: 0,
            door: Door::Web(from.host.clone()),
        };
        tokio::spawn(visit(
            shared.clone(),
            service.to_string(),
            svc,
            crate::forward::Incoming::Web(theirs),
            entry,
            admitted,
        ));
        Ok(Box::new(ours))
    }

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Permit> {
        super::listen::try_permit(&self.0, peer)
    }

    async fn tunnel(&self, conn: TunnelConn<Permit>) {
        super::listen::handle_web_tunnel(self.0.clone(), conn).await;
    }
}

pub(super) fn refusal(outcome: &Outcome) -> Option<Refused> {
    let status = |st: Status| match st {
        Status::Busy => Refused::Full,
        Status::Offline => Refused::Down,
        Status::Unreachable | Status::Unresolved => Refused::Unreachable,
        Status::Denied | Status::Forbidden | Status::Invalid | Status::Ok => Refused::Denied,
    };
    match outcome {
        Outcome::Done {
            result: Err(ForwardError::Status(st)),
            ..
        }
        | Outcome::Refused {
            error: ForwardError::Status(st),
            ..
        } => Some(status(*st)),
        Outcome::Full(_) => Some(Refused::Full),
        Outcome::Unavailable(Status::Busy) => Some(Refused::Full),
        Outcome::Unavailable(_) => Some(Refused::Down),
        _ => None,
    }
}

pub(super) fn proxy_settings(s: &ServerSettings) -> Result<Option<ProxySettings>, Failure> {
    let Some(web) = &s.web else {
        return Ok(None);
    };
    let mut p = ProxySettings {
        https: web.https,
        http: web.http,
        ..ProxySettings::default()
    };
    p.sites = web
        .sites
        .iter()
        .map(|(service, site)| Site {
            service: service.clone(),
            hosts: site.hosts.clone(),
            tls: site_tls(&site.tls),
            https_redirect: site.https_redirect,
        })
        .collect();
    p.max_pending = s.max_handshakes;
    p.timeouts.handshake = s.handshake_timeout;
    p.timeouts.request_header = web.request_header_timeout;
    p.timeouts.idle = web.idle_timeout;
    p.certs = cert_settings(s, web);
    if let Some(ws) = s.websocket.as_ref()
        && let Some(host) = &ws.host
    {
        p.tunnel = Some(TunnelSettings {
            host: host.clone(),
            path: ws.path.clone(),
            mode: tunnel_mode(web, host, ws.shares),
            port: match ws.shares {
                Shares::Http => TunnelPort::Http,
                Shares::Https | Shares::Nothing => TunnelPort::Https,
            },
        });
    }
    Ok(Some(p))
}

fn tunnel_mode(web: &WebSettings, host: &str, shares: Shares) -> TunnelMode {
    let on_site = web.sites.values().any(|site| {
        let served = match site.tls {
            WebTls::Auto | WebTls::Files { .. } => true,
            WebTls::Off => shares == Shares::Http,
            WebTls::Passthrough => false,
        };
        served && site.hosts.iter().any(|h| h == host)
    });
    if on_site {
        TunnelMode::ByPath
    } else {
        TunnelMode::OwnName
    }
}

fn site_tls(tls: &WebTls) -> SiteTls {
    match tls {
        WebTls::Passthrough => SiteTls::Passthrough,
        WebTls::Auto => SiteTls::Https(CertSource::Auto),
        WebTls::Files { cert, key } => SiteTls::Https(CertSource::OwnFile {
            cert: cert.clone(),
            key: key.clone(),
        }),
        WebTls::Off => SiteTls::Plain,
    }
}

fn cert_settings(s: &ServerSettings, web: &WebSettings) -> CertSettings {
    let tunnel_file = s
        .websocket
        .as_ref()
        .filter(|ws| ws.host.is_some())
        .and_then(|ws| ws.tls.as_ref())
        .map(|t| (t.cert_path.clone(), t.key_path.clone()));
    CertSettings {
        shared: web
            .certificates
            .iter()
            .map(|c| SharedFile {
                hosts: c.hosts.clone(),
                cert: c.cert.clone(),
                key: c.key.clone(),
            })
            .collect(),
        tunnel_file,
        acme: web.letsencrypt.as_ref().map(|l| AcmeSettings {
            directory_url: l.directory.clone(),
            ca: l.ca.clone(),
            email: l.email.clone(),
            profile: l.profile.clone(),
            state_dir: l.state_dir.clone(),
            check_ports: burrow_syntax::web::CheckPorts {
                https: web.https.map(|a| a.port()),
                http: web.http.map(|a| a.port()),
            },
            dns: false,
            fallback_directory: (l.directory == burrow_web::tls::acme::LETSENCRYPT_PRODUCTION)
                .then(|| burrow_web::tls::acme::LETSENCRYPT_STAGING.to_string()),
        }),
    }
}

mod task;

pub(super) use task::WebTask;
