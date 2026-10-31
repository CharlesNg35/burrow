use std::sync::Arc;

use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::warn;

use super::pages::ErrorPages;
use super::upstream::target_tls::{Check, connector, server_name};
use crate::auth::{Limit, Users};
use crate::{Site, SiteOptions, TargetTls};

#[derive(Clone)]
pub enum Hop {
    Plain,
    Tls(TlsConnector, ServerName<'static>),
    Broken,
}

pub struct SiteRun {
    pub service: String,
    pub options: SiteOptions,
    pub pages: ErrorPages,
    pub hop: Hop,
    pub(crate) users: Users,
    pub limit: Option<Arc<Limit>>,
}

impl std::fmt::Debug for SiteRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SiteRun")
            .field("service", &self.service)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl PartialEq for SiteRun {
    fn eq(&self, other: &SiteRun) -> bool {
        self.service == other.service && self.options == other.options
    }
}

impl Eq for SiteRun {}

fn check(t: &TargetTls) -> Result<Option<Check>, String> {
    Ok(match t {
        TargetTls::Off => None,
        TargetTls::PublicRoots => Some(Check::PublicRoots),
        TargetTls::Ca(path) => {
            Some(Check::OwnCa(std::fs::read(path).map_err(|e| {
                format!("cannot read {}: {e}", path.display())
            })?))
        }
        TargetTls::Unchecked => Some(Check::Unchecked),
    })
}

pub(crate) fn hop(o: &SiteOptions) -> Result<Hop, String> {
    let Some(check) = check(&o.target_tls)? else {
        return Ok(Hop::Plain);
    };
    Ok(Hop::Tls(
        connector(&check, o.target_http2)?,
        server_name(&o.target_name)?,
    ))
}

fn limit(o: &SiteOptions, old: Option<&SiteRun>) -> Option<Arc<Limit>> {
    let rate = o.rate_limit?;
    match old.and_then(|s| s.limit.as_ref()) {
        Some(l) if l.rate() == rate => Some(l.clone()),
        _ => Some(Arc::new(Limit::per_second(rate))),
    }
}

impl SiteRun {
    pub fn build(site: &Site) -> Arc<SiteRun> {
        SiteRun::build_after(site, None)
    }

    pub fn build_after(site: &Site, old: Option<&SiteRun>) -> Arc<SiteRun> {
        let o = &site.options;
        let pages = o
            .error_pages
            .as_deref()
            .map_or_else(|| Ok(ErrorPages::default()), ErrorPages::load);
        let pages = pages.unwrap_or_else(|e| {
            warn!("web: service \"{}\": error_pages: {e}", site.service);
            ErrorPages::default()
        });
        let hop = hop(o).unwrap_or_else(|e| {
            warn!("web: service \"{}\": target_tls: {e}", site.service);
            Hop::Broken
        });
        Arc::new(SiteRun {
            service: site.service.clone(),
            options: o.clone(),
            pages,
            hop,
            users: Users::build(&site.service, &o.basic_auth),
            limit: limit(o, old),
        })
    }
}
