use std::net::SocketAddr;
use std::sync::Arc;

use burrow_web::{
    AcmeSettings, Admission, BoxIo, CertSettings, CertSource, ProxySettings, Refused, SharedFile,
    Site, SiteTls, TunnelConn, TunnelMode, TunnelPort, TunnelSettings, Upstream, Visitor,
};
use tokio::net::TcpStream;
use tracing::debug;

use super::Shared;
use super::listen::Permit;
use super::route::Outcome;
use super::tcp::{Door, Entry, Refusal, Shape, admit, visit};
use crate::config::settings::{ServerSettings, Shares, WebSettings, WebTls};
use crate::forward::ForwardError;
use crate::policy::geo::Verdict;
use crate::wire::protocol::Status;

pub(super) struct Doorway(Arc<Shared>);

impl Upstream for Doorway {
    type Permit = Permit;

    fn answered(&self, service: &str, status: http::StatusCode) {
        self.0.metrics.service(service).answered(status.as_u16());
    }

    fn tune(&self, stream: &TcpStream) {
        crate::wire::transport::tcp::tune(stream, &self.0.settings().tcp);
    }

    fn refused(&self) {
        self.0.refused_handshake();
    }

    fn allowed(&self, service: &str, from: &Visitor) -> Admission {
        match self.0.admission(service, from.addr.ip()) {
            Verdict::In => Admission::In,
            Verdict::Address => Admission::Refused,
            Verdict::Region => Admission::Region {
                credit: self
                    .0
                    .settings()
                    .geoip
                    .as_ref()
                    .is_some_and(crate::config::settings::GeoSource::credits_db_ip),
            },
        }
    }

    async fn open(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        self.0
            .still_on_web(service, &from.host)
            .map_err(|_| Refused::Gone)?;
        open_door(&self.0, service, from, Door::Web(from.host.clone()))
    }

    async fn open_auth(&self, service: &str, from: &Visitor) -> Result<BoxIo, Refused> {
        open_door(&self.0, service, from, Door::Auth)
    }

    fn tunnel_permit(&self, peer: SocketAddr) -> Option<Permit> {
        super::listen::try_permit(&self.0, peer)
    }

    async fn tunnel(&self, conn: TunnelConn<Permit>) {
        super::listen::handle_web_tunnel(self.0.clone(), conn).await;
    }
}

fn open_door(
    shared: &Arc<Shared>,
    service: &str,
    from: &Visitor,
    door: Door,
) -> Result<BoxIo, Refused> {
    let settings = shared.settings();
    let svc = settings.services.get(service).ok_or(Refused::Gone)?;
    let verdict = shared.admission(service, from.addr.ip());
    if !verdict.admitted() {
        debug!(peer = %from.addr, "service \"{service}\": {}, closing", super::tcp::refused_why(verdict));
        return Err(Refused::NotAllowed);
    }
    let source = super::share::web_source(from.addr);
    let admitted = admit(shared, service, svc, (from.addr, source), 0).map_err(|r| match r {
        Refusal::Full => Refused::Full,
        Refusal::NoClient => Refused::NoClient,
    })?;
    let (ours, theirs) = burrow_web::pipe();
    theirs.set_reset(true);
    let entry = Entry {
        peer: from.addr,
        local: Some(from.local),
        index: 0,
        door,
    };
    let shape = Shape::of(svc, &entry);
    tokio::spawn(visit(
        shared.clone(),
        service.to_string(),
        shape,
        crate::forward::Incoming::Web(theirs),
        entry,
        admitted,
    ));
    Ok(Box::new(ours))
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

pub(super) fn proxy_settings(s: &ServerSettings) -> Option<ProxySettings> {
    let web = s.web.as_ref()?;
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
            options: options::site_options(&site.options),
        })
        .collect();
    p.max_pending = s.max_handshakes;
    p.timeouts.handshake = s.handshake_timeout;
    p.timeouts.request_header = web.request_header_timeout;
    p.timeouts.idle = web.idle_timeout;
    p.certs = cert_settings(s, web);
    options::site_wide(web, &mut p);
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
    Some(p)
}

fn tunnel_mode(web: &WebSettings, host: &str, shares: Shares) -> TunnelMode {
    let on_site = web.sites.values().any(|site| {
        let served = match site.tls {
            WebTls::Auto | WebTls::Files { .. } | WebTls::Internal => true,
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
        WebTls::Internal => SiteTls::Https(CertSource::Internal),
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
        choices: web
            .sites
            .iter()
            .filter_map(|(name, site)| site.challenge.map(|c| (name.clone(), c)))
            .chain(
                web.tunnel_challenge
                    .map(|c| (burrow_web::tls::TUNNEL_GROUP.to_string(), c)),
            )
            .collect(),
        state_dir: web.state_dir.clone(),
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
            dns: options::dns_entries(&l.dns),
            fallback_directory: (l.directory == burrow_web::tls::acme::LETSENCRYPT_PRODUCTION)
                .then(|| burrow_web::tls::acme::LETSENCRYPT_STAGING.to_string()),
        }),
    }
}

mod health;
mod options;
mod task;

pub(super) use health::{Checks, Http, exchange};
pub(super) use task::WebTask;
