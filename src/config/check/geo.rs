use std::collections::BTreeMap;
use std::path::Path;

use burrow_admin_proto::codes::{
    COUNTRIES_BOTH, COUNTRIES_UNUSED, GEOIP_INVALID, GEOIP_NOT_READY, GEOIP_UNUSED, INVALID_COUNTRY,
};
use burrow_config::{ServerConfig, ServiceEntry};

use super::Spans;
use crate::config::diagnostics::Diagnostic;
use crate::config::settings::{DB_IP, DB_IP_FILE, DB_IP_FOLDER, GeoSource, Service};
use crate::policy::geo::Countries;

pub(super) fn countries(
    name: &str,
    svc: &ServiceEntry,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<Countries> {
    let field = |key: &'static str| ["services", name, key];
    if !svc.allow_countries.is_empty() && !svc.deny_countries.is_empty() {
        out.push(
            Diagnostic::error(
                COUNTRIES_BOTH,
                format!("service \"{name}\" has both allow_countries and deny_countries"),
            )
            .at(spans.key(&field("deny_countries")))
            .label("only one of the two")
            .help("keep allow_countries to let in only those countries, or deny_countries to keep those out"),
        );
        return None;
    }
    let (key, list, make): (_, _, fn(_) -> Countries) = if svc.deny_countries.is_empty() {
        ("allow_countries", &svc.allow_countries, Countries::Allow)
    } else {
        ("deny_countries", &svc.deny_countries, Countries::Deny)
    };
    if list.is_empty() {
        return None;
    }
    let mut codes = std::collections::BTreeSet::new();
    for (i, text) in list.iter().enumerate() {
        match burrow_syntax::country::country(text) {
            Ok(code) => {
                codes.insert(code);
            }
            Err(e) => {
                let n = i.to_string();
                out.push(
                    Diagnostic::error(
                        INVALID_COUNTRY,
                        format!("invalid {key} entry for service \"{name}\": {e}"),
                    )
                    .at(spans.value(&["services", name, key, &n]))
                    .label("expected a two-letter country code")
                    .help("use ISO 3166 codes such as FR, DE, CH or US; burrow admin shows the full list"),
                );
            }
        }
    }
    if svc.bind.is_none() && svc.web.is_none() {
        out.push(
            Diagnostic::warning(
                COUNTRIES_UNUSED,
                format!("{key} of service \"{name}\" has no effect: it has no public port and no website"),
            )
            .at(spans.key(&field(key)))
            .label("nothing to filter")
            .help("country rules apply to bind and web only; burrow visit and clients are not judged by country"),
        );
    }
    Some(make(codes))
}

pub(super) fn source(
    cfg: &ServerConfig,
    services: &BTreeMap<String, Service>,
    (base_dir, state): (&Path, Option<&Path>),
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) -> Option<GeoSource> {
    let at = || spans.at(&[], ServerConfig::GEOIP);
    let used = services.values().any(|s| s.countries.is_some());
    let given = cfg.geoip.as_deref().map(str::trim);
    if !used {
        if given.is_some() {
            out.push(
                Diagnostic::warning(GEOIP_UNUSED, "geoip is set, but no service has allow_countries or deny_countries")
                    .at(at())
                    .label("not used")
                    .help("add allow_countries or deny_countries to a service, or remove geoip; nothing is downloaded until a service uses it"),
            );
        }
        return None;
    }
    let strict = services
        .iter()
        .filter(|(_, s)| matches!(s.countries, Some(Countries::Allow(_))))
        .map(|(n, _)| format!("\"{n}\""))
        .collect::<Vec<_>>();
    let source = match given {
        None | Some(DB_IP) => GeoSource::DbIp {
            dir: state.map(|s| s.join(DB_IP_FOLDER)),
        },
        Some(text) if text.contains("://") => {
            out.push(
                Diagnostic::error(GEOIP_INVALID, format!("geoip \"{text}\" is a URL"))
                    .at(at())
                    .label("expected db-ip or a file")
                    .help("use db-ip for the automatic monthly DB-IP Lite download, or the path of an .mmdb file on this server"),
            );
            return None;
        }
        Some(text) => GeoSource::File(base_dir.join(text)),
    };
    let file = match &source {
        GeoSource::DbIp { dir } => dir.as_ref().map(|d| d.join(DB_IP_FILE)),
        GeoSource::File(p) => Some(p.clone()),
    };
    let problem = match file.as_deref() {
        None => Some("there is no state folder to download it into".to_string()),
        Some(f) => readable(f).err(),
    };
    if let Some(why) = problem {
        let (label, help) = match &source {
            GeoSource::DbIp { .. } => (
                "downloads on start",
                "the server downloads DB-IP Lite from download.db-ip.com when it starts; until then",
            ),
            GeoSource::File(_) => (
                "file not ready",
                "put a MaxMind-format country .mmdb file there; until then",
            ),
        };
        let refusing = match strict.as_slice() {
            [] => "deny_countries lets every visitor in".to_string(),
            [one] => format!("service {one} refuses public visitors (allow_countries)"),
            many => format!(
                "services {} refuse public visitors (allow_countries)",
                many.join(", ")
            ),
        };
        let d = Diagnostic::warning(
            GEOIP_NOT_READY,
            format!("the country database is not ready: {why}"),
        )
        .help(format!("{help}, {refusing}"));
        out.push(if spans.has_at(&[], ServerConfig::GEOIP) {
            d.at(at()).label(label)
        } else {
            d.without_snippet()
        });
    }
    Some(source)
}

pub fn readable(path: &Path) -> Result<String, String> {
    if !path.exists() {
        return Err(format!("{} does not exist yet", path.display()));
    }
    let reader = maxminddb::Reader::open_readfile(path)
        .map_err(|e| format!("{} is not a readable .mmdb file: {e}", path.display()))?;
    let kind = reader.metadata().database_type.clone();
    if !kind.to_ascii_lowercase().contains("country") && !kind.to_ascii_lowercase().contains("city")
    {
        return Err(format!(
            "{} is a {kind} database, not a country database",
            path.display()
        ));
    }
    Ok(kind)
}
