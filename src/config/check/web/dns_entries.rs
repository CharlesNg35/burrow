use std::path::Path;

use super::Spans;
use super::fill_dns::{FILE_PREFIX, resolver};
use crate::config::diagnostics::Diagnostic;
use burrow_config::{DnsEntry, ServerConfig};
use burrow_syntax::web::{SingleLabel, dns_entry_for, host_name, is_wildcard};

use burrow_syntax::web::{DNS_WAIT_MAX_SECS, DNS_WAIT_MIN_SECS, DNS_WAIT_SECS};

pub(super) fn check_dns_entries(
    cfg: &ServerConfig,
    base_dir: &Path,
    spans: &Spans,
    out: &mut Vec<Diagnostic>,
) {
    let Some(le) = cfg.web.as_ref().and_then(|w| w.letsencrypt.as_ref()) else {
        return;
    };
    let mut domains: Vec<(String, usize)> = Vec::new();
    for (i, e) in le.dns.iter().enumerate() {
        entry(i, e, base_dir, spans, &mut domains, out);
    }
    wildcards(cfg, &le.dns, spans, out);
}

fn at(i: usize, rest: &[&str]) -> Vec<String> {
    let mut p = vec![
        "web".to_string(),
        "letsencrypt".into(),
        "dns".into(),
        i.to_string(),
    ];
    p.extend(rest.iter().map(|s| s.to_string()));
    p
}

fn span(spans: &Spans, path: &[String]) -> crate::config::tree::Span {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    spans.value(&p)
}

fn entry(
    i: usize,
    e: &DnsEntry,
    base_dir: &Path,
    spans: &Spans,
    domains: &mut Vec<(String, usize)>,
    out: &mut Vec<Diagnostic>,
) {
    let name = format!("web.letsencrypt.dns[{i}]");
    let count = [
        e.cloudflare.is_some(),
        e.rfc2136.is_some(),
        e.acme_dns.is_some(),
        e.command.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if count != 1 {
        out.push(
            Diagnostic::error(
                "dns-provider",
                format!("{name}: name exactly one of cloudflare, rfc2136, acme_dns or command"),
            )
            .at(span(spans, &at(i, &[])))
            .help(
                "each entry sets records through one provider; add another entry for other domains",
            ),
        );
    }
    if e.domains.is_empty() {
        out.push(
            Diagnostic::error(
                "dns-domains",
                format!("{name}.domains is required: the domains this provider manages, like domains: [example.com]"),
            )
            .at(span(spans, &at(i, &[])))
            .help("a domain covers itself and every name under it"),
        );
    }
    for (j, text) in e.domains.iter().enumerate() {
        let here = span(spans, &at(i, &["domains", &j.to_string()]));
        let problem = match host_name(text, SingleLabel::Allowed) {
            Err(e) => Some(format!("{name}.domains: {e}")),
            Ok(d) if is_wildcard(&d) => Some(format!(
                "{name}.domains: write the domain {}; it already covers {d} and every name under it",
                &d[2..]
            )),
            Ok(d) => {
                if let Some((_, first)) = domains.iter().find(|(x, _)| *x == d) {
                    Some(format!(
                        "domain \"{d}\" is listed in web.letsencrypt.dns[{first}] and [{i}]; list it in one entry only"
                    ))
                } else {
                    domains.push((d, i));
                    None
                }
            }
        };
        if let Some(m) = problem {
            out.push(
                Diagnostic::error("dns-domains", m)
                    .at(here)
                    .help("for example domains: [example.com]"),
            );
        }
    }
    if let Some(w) = e.wait_secs
        && !(DNS_WAIT_MIN_SECS..=DNS_WAIT_MAX_SECS).contains(&w)
    {
        out.push(
            Diagnostic::error(
                "dns-wait",
                format!(
                    "{name}.wait_secs must be between {DNS_WAIT_MIN_SECS} and {DNS_WAIT_MAX_SECS}"
                ),
            )
            .at(span(spans, &at(i, &["wait_secs"])))
            .help(format!("{DNS_WAIT_SECS} when not set, like Caddy")),
        );
    }
    for (j, r) in e.resolvers.iter().enumerate() {
        if resolver(r).is_none() {
            out.push(
                Diagnostic::error(
                    "dns-resolver",
                    format!(
                        "{name}.resolvers: \"{}\" is not an IP address or ip:port",
                        r.trim()
                    ),
                )
                .at(span(spans, &at(i, &["resolvers", &j.to_string()])))
                .help("for example resolvers: [1.1.1.1, 9.9.9.9]"),
            );
        }
    }
    provider(i, e, base_dir, spans, out);
}

fn provider(i: usize, e: &DnsEntry, base_dir: &Path, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let name = format!("web.letsencrypt.dns[{i}]");
    if let Some(r) = &e.rfc2136 {
        let server = r.server.trim();
        if let Err(why) = burrow_syntax::hostport::parse_host_port(server) {
            let text = match server.trim_end_matches(':').contains(':') {
                true => format!("{name}.rfc2136.server: {why}"),
                false => format!(
                    "{name}.rfc2136: server \"{server}\" needs a port, like {}:53",
                    server.trim_end_matches(':')
                ),
            };
            out.push(
                Diagnostic::error("dns-rfc2136", text)
                    .at(span(spans, &at(i, &["rfc2136", "server"])))
                    .help("the zone's primary nameserver, usually on port 53, with an IPv6 address in brackets like [2001:db8::53]:53"),
            );
        }
        let alg = tsig(r.algorithm.as_deref());
        if let Some(Err(m)) = &alg {
            out.push(
                Diagnostic::error("dns-rfc2136", format!("{name}.rfc2136.algorithm: {m}"))
                    .at(span(spans, &at(i, &["rfc2136", "algorithm"])))
                    .help("write the algorithm of the server's key file; hmac-md5 and hmac-sha1 are not safe enough (RFC 8945 section 6)"),
            );
        }
        let wanted = alg.and_then(Result::ok);
        if let Some(secret) = super::fill_dns::secret(&r.secret, base_dir) {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(secret.expose()) {
                Err(_) => out.push(
                    Diagnostic::error(
                        "dns-rfc2136",
                        format!("{name}.rfc2136.secret is not base64"),
                    )
                    .at(span(spans, &at(i, &["rfc2136", "secret"])))
                    .help("copy the secret from the server's key file, as tsig-keygen writes it"),
                ),
                Ok(bytes) => {
                    if let Some((alg, mac)) = wanted.filter(|(_, mac)| bytes.len() < *mac) {
                        out.push(
                            Diagnostic::warning("dns-rfc2136", format!("{name}.rfc2136.secret is {} bytes; RFC 8945 asks for at least {mac} for {alg}", bytes.len()))
                                .at(span(spans, &at(i, &["rfc2136", "secret"])))
                                .help(format!("make a new key with tsig-keygen -a {alg}")),
                        );
                    }
                }
            }
        }
    }
    if let Some(a) = &e.acme_dns {
        acme_dns_api(i, a.api.trim(), spans, out);
    }
    if let Some(c) = &e.command {
        command(i, c.trim(), base_dir, spans, out);
    }
}

#[cfg(feature = "web")]
fn tsig(written: Option<&str>) -> Option<Result<(&'static str, usize), String>> {
    use burrow_web::TsigAlgorithm;
    let found = match written {
        None => Some(TsigAlgorithm::default()),
        Some(w) => TsigAlgorithm::parse(w),
    };
    Some(found.map(|a| (a.name(), a.mac_bytes())).ok_or_else(|| {
        let names: Vec<&str> = TsigAlgorithm::ALL.iter().map(|a| a.name()).collect();
        let (last, rest) = names.split_last().expect("three algorithms");
        format!(
            "\"{}\" is not {} or {last}",
            written.unwrap_or_default().trim(),
            rest.join(", ")
        )
    }))
}

#[cfg(not(feature = "web"))]
fn tsig(_: Option<&str>) -> Option<Result<(&'static str, usize), String>> {
    None
}

#[cfg(feature = "web")]
fn public_ip(host: &str) -> Option<std::net::IpAddr> {
    host.trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>()
        .ok()
        .filter(|ip| !burrow_web::tls::acme::http::is_private(*ip))
}

#[cfg(not(feature = "web"))]
fn public_ip(_: &str) -> Option<std::net::IpAddr> {
    None
}

fn acme_dns_api(i: usize, api: &str, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let uri = api
        .parse::<http::Uri>()
        .ok()
        .filter(|u| u.host().is_some_and(|h| !h.is_empty()));
    let problem = match uri.as_ref().map(|u| (u.scheme_str(), u.host())) {
        Some((Some("https"), _)) => None,
        Some((Some("http"), Some(host))) => public_ip(host).map(|ip| {
            format!("{ip} is a public address; plain http:// is allowed only to loopback or private addresses, so use https://")
        }),
        _ => Some(format!(
            "\"{api}\" is not an https:// address, like https://auth.example.org"
        )),
    };
    if let Some(m) = problem {
        out.push(
            Diagnostic::error("dns-acme-dns", format!("web.letsencrypt.dns[{i}].acme_dns.api: {m}"))
                .at(span(spans, &at(i, &["acme_dns", "api"])))
                .help("plain http:// suits an acme-dns server on your own network, like http://192.168.1.5:8080"),
        );
    }
}

fn command(i: usize, c: &str, base_dir: &Path, spans: &Spans, out: &mut Vec<Diagnostic>) {
    let here = span(spans, &at(i, &["command"]));
    if let Some(rest) = c.strip_prefix(FILE_PREFIX) {
        out.push(
            Diagnostic::error(
                "dns-command",
                format!("web.letsencrypt.dns[{i}].command: {c} is the path of a program, not a secret, so file: does not belong there"),
            )
            .at(here)
            .help(format!("write the path itself: command: {}", rest.trim())),
        );
        return;
    }
    let path = base_dir.join(c);
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file());
    #[cfg(unix)]
    let runnable = meta.as_ref().is_some_and(|m| {
        use std::os::unix::fs::PermissionsExt as _;
        m.permissions().mode() & 0o111 != 0
    });
    #[cfg(not(unix))]
    let runnable = meta.is_some();
    if !runnable {
        out.push(
            Diagnostic::error(
                "dns-command",
                format!("web.letsencrypt.dns[{i}].command: {c} does not exist or is not executable"),
            )
            .at(here)
            .help("give the full path of a program burrow's user can run, like /usr/local/bin/burrow-dns"),
        );
    }
    #[cfg(unix)]
    if runnable
        && meta.is_some_and(|m| {
            use std::os::unix::fs::PermissionsExt as _;
            m.permissions().mode() & 0o022 != 0
        })
    {
        out.push(
            Diagnostic::warning(
                "dns-command",
                format!("web.letsencrypt.dns[{i}].command: {c} can be changed by users other than its owner"),
            )
            .at(here)
            .help("burrow runs it to set DNS records; remove group and other write access, like chmod go-w"),
        );
    }
}

fn wildcards(cfg: &ServerConfig, list: &[DnsEntry], spans: &Spans, out: &mut Vec<Diagnostic>) {
    let entries = || {
        list.iter()
            .enumerate()
            .map(|(i, e)| (i, e.domains.as_slice()))
    };
    let mut seen: Vec<(String, usize)> = Vec::new();
    for (i, e) in list.iter().enumerate() {
        let name = format!("web.letsencrypt.dns[{i}].wildcards");
        for (j, text) in e.wildcards.iter().enumerate() {
            let here = span(spans, &at(i, &["wildcards", &j.to_string()]));
            let problem = match host_name(text, SingleLabel::Allowed) {
                Err(e) => Some(format!("{name}: {e}")),
                Ok(w) if !is_wildcard(&w) => Some(format!(
                    "{name}: \"{w}\" is not a wildcard; write it as \"*.{w}\""
                )),
                Ok(w) => match dns_entry_for(entries(), &w) {
                    None => Some(format!(
                        "{name}: \"{w}\" is not under this entry's domains ({})",
                        e.domains.join(", ")
                    )),
                    Some(k) if k != i => Some(format!(
                        "{name}: \"{w}\" belongs to {}, which dns[{k}] manages; list it in dns[{k}].wildcards",
                        &w[2..]
                    )),
                    Some(_) => match seen.iter().find(|(x, _)| *x == w) {
                        Some((_, first)) => Some(format!(
                            "wildcard \"{w}\" is listed in web.letsencrypt.dns[{first}] and [{i}]; list it once"
                        )),
                        None => {
                            if !used(cfg, &w) {
                                out.push(
                                    Diagnostic::warning("dns-wildcard", format!("wildcard {w} is used by nothing yet"))
                                        .at(here)
                                        .help("it is still ordered and renewed, ready for sites added later"),
                                );
                            }
                            seen.push((w, i));
                            None
                        }
                    },
                },
            };
            if let Some(m) = problem {
                out.push(
                    Diagnostic::error("dns-wildcard", m)
                        .at(here)
                        .help("a shared wildcard like *.example.com also covers example.com"),
                );
            }
        }
    }
}

fn used(cfg: &ServerConfig, wildcard: &str) -> bool {
    let parent = &wildcard[2..];
    cfg.services
        .values()
        .filter_map(|s| s.web.as_ref())
        .any(|w| {
            w.hosts()
                .iter()
                .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
                .any(|h| h == wildcard || h == parent || burrow_syntax::web::covers(wildcard, &h))
        })
}

pub(super) fn wildcard_hint(cfg: &ServerConfig, spans: &Spans, out: &mut Vec<Diagnostic>) {
    use std::collections::BTreeMap;
    let Some(web) = cfg.web.as_ref() else {
        return;
    };
    let Some(le) = web.letsencrypt.as_ref().filter(|l| !l.dns.is_empty()) else {
        return;
    };
    let files: Vec<&[String]> = web
        .certificates
        .iter()
        .map(|c| c.hosts.as_slice())
        .collect();
    let listed: Vec<String> = le
        .dns
        .iter()
        .flat_map(|e| e.wildcards.iter())
        .chain(
            cfg.services
                .values()
                .filter_map(|s| s.web.as_ref())
                .flat_map(|w| w.hosts().iter()),
        )
        .filter_map(|h| host_name(h, SingleLabel::Allowed).ok())
        .filter(|h| is_wildcard(h))
        .collect();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for svc in cfg.services.values() {
        let Some(w) = svc.web.as_ref() else {
            continue;
        };
        if !matches!(
            w.tls(),
            None | Some(burrow_config::SiteTls::Mode(burrow_config::TlsMode::Auto))
        ) {
            continue;
        }
        for h in w
            .hosts()
            .iter()
            .filter_map(|h| host_name(h, SingleLabel::Refused).ok())
        {
            let Some((first, parent)) = h.split_once('.') else {
                continue;
            };
            let covered = listed.iter().any(|w| burrow_syntax::web::covers(w, &h))
                || !matches!(
                    burrow_syntax::web::entry_for(files.iter().copied().enumerate(), &h),
                    Ok(None)
                );
            if is_wildcard(&h) || burrow_syntax::web::is_ip(&h) || covered || !parent.contains('.')
            {
                continue;
            }
            let entries = le
                .dns
                .iter()
                .enumerate()
                .map(|(i, e)| (i, e.domains.as_slice()));
            if dns_entry_for(entries, parent).is_some() {
                let g = groups.entry(parent.to_string()).or_default();
                if !g.iter().any(|x| x == first) {
                    g.push(first.to_string());
                }
            }
        }
    }
    for (parent, labels) in groups.into_iter().filter(|(_, l)| l.len() >= 3) {
        let entries = le
            .dns
            .iter()
            .enumerate()
            .map(|(i, e)| (i, e.domains.as_slice()));
        let Some(i) = dns_entry_for(entries, &parent) else {
            continue;
        };
        let domain = le.dns[i]
            .domains
            .iter()
            .filter_map(|d| host_name(d, SingleLabel::Allowed).ok())
            .filter(|d| parent == *d || parent.ends_with(&format!(".{d}")))
            .max_by_key(String::len)
            .unwrap_or_default();
        let (last, rest) = labels.split_last().expect("three or more");
        out.push(
            Diagnostic::warning(
                "wildcard-hint",
                format!(
                    "{} and {last}.{parent} each get their own certificate; one wildcard would cover them: add \"*.{parent}\" to wildcards in the letsencrypt.dns entry for {domain}",
                    rest.join(", ")
                ),
            )
            .at(spans.value(&["web", "letsencrypt", "dns", &i.to_string()]))
            .help("one shared certificate means one order and one renewal instead of one per site"),
        );
    }
}
