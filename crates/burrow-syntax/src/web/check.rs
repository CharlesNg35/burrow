use super::{is_ip, is_wildcard, sni_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Dns,
    Tls,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Challenge {
    Dns,
    Http,
    Tls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCheck {
    DnsNotSet,
    DnsForIp,
    NoPort,
    NoHttpPort,
    NoTlsPort,
    WildcardNotDns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckPorts {
    pub https: Option<u16>,
    pub http: Option<u16>,
}

pub const TLS_PORT: u16 = 443;
pub const HTTP_PORT: u16 = 80;

pub fn checks_for(
    name: &str,
    ports: CheckPorts,
    dns: bool,
    choice: Option<Challenge>,
) -> Result<Vec<Check>, NoCheck> {
    let name = sni_name(name.trim());
    let tls = ports.https == Some(TLS_PORT);
    let http = ports.http == Some(HTTP_PORT);
    let dns_check = || match (is_ip(&name), dns) {
        (true, _) => Err(NoCheck::DnsForIp),
        (false, false) => Err(NoCheck::DnsNotSet),
        (false, true) => Ok(vec![Check::Dns]),
    };
    if is_wildcard(&name) {
        return match choice {
            Some(Challenge::Http | Challenge::Tls) => Err(NoCheck::WildcardNotDns),
            Some(Challenge::Dns) | None => dns_check(),
        };
    }
    match choice {
        Some(Challenge::Dns) => dns_check(),
        Some(Challenge::Tls) if tls => Ok(vec![Check::Tls]),
        Some(Challenge::Tls) => Err(NoCheck::NoTlsPort),
        Some(Challenge::Http) if http => Ok(vec![Check::Http]),
        Some(Challenge::Http) => Err(NoCheck::NoHttpPort),
        None => {
            let mut out = Vec::new();
            if tls {
                out.push(Check::Tls);
            }
            if http {
                out.push(Check::Http);
            }
            if out.is_empty() {
                return Err(NoCheck::NoPort);
            }
            Ok(out)
        }
    }
}

pub fn dns_entry_for<'a, D>(
    entries: impl IntoIterator<Item = (usize, &'a [D])>,
    name: &str,
) -> Option<usize>
where
    D: AsRef<str> + 'a,
{
    let name = sni_name(name.trim());
    let name = name.strip_prefix("*.").unwrap_or(&name);
    let mut best: Option<(usize, usize)> = None;
    for (i, domains) in entries {
        for d in domains {
            let d = sni_name(d.as_ref().trim());
            let under = name == d
                || name
                    .strip_suffix(d.as_str())
                    .is_some_and(|r| r.ends_with('.'));
            if under && !d.is_empty() && best.is_none_or(|(_, len)| d.len() > len) {
                best = Some((i, d.len()));
            }
        }
    }
    best.map(|(i, _)| i)
}

pub struct AutoName<'a> {
    pub label: &'a str,
    pub host: &'a str,
    pub choice: Option<Challenge>,
}

pub const PROXY_NOTE: &str = "a proxy is in front (web.trusted_proxies is set); if it does the HTTPS, like Cloudflare's proxy, Let's Encrypt's TLS check cannot reach burrow: set challenge: http (with http: 80) or challenge: dns on these sites: ";

pub fn proxy_note(names: &[AutoName<'_>], ports: CheckPorts, dns: bool) -> Option<String> {
    let mut labels: Vec<&str> = Vec::new();
    for n in names {
        if n.choice.is_some() {
            continue;
        }
        let tls_first =
            checks_for(n.host, ports, dns, None).is_ok_and(|c| c.first() == Some(&Check::Tls));
        if tls_first && !labels.contains(&n.label) {
            labels.push(n.label);
        }
    }
    (!labels.is_empty()).then(|| format!("{PROXY_NOTE}{}", labels.join(", ")))
}
