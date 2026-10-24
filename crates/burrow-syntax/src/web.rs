use std::net::IpAddr;

pub const MAX_HOST_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleLabel {
    Allowed,
    Refused,
}

pub fn host_name(text: &str, single: SingleLabel) -> Result<String, String> {
    let t = text.trim();
    let bare = t
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(t);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return Ok(ip.to_canonical().to_string());
    }
    if t.is_empty() {
        return Err("a host name is required, like blog.example.com".into());
    }
    if t.contains("://") || t.contains('/') || t.contains(':') || t.contains(' ') {
        return Err(format!(
            "\"{text}\" is not a host name; write the name alone, like blog.example.com"
        ));
    }
    let name = t.strip_suffix('.').unwrap_or(t).to_ascii_lowercase();
    let labels: Vec<&str> = name.split('.').collect();
    let wildcard = labels[0] == "*";
    if labels.iter().skip(1).any(|l| l.contains('*')) || (labels[0].contains('*') && !wildcard) {
        return Err(format!(
            "\"{text}\" is not a valid wildcard; a wildcard is \"*.\" followed by a name, like *.example.com"
        ));
    }
    if wildcard && labels.len() < 3 {
        return Err(format!(
            "\"{text}\" is too wide; a wildcard needs a parent name, like *.example.com"
        ));
    }
    if labels.len() < 2 && single == SingleLabel::Refused {
        return Err(format!(
            "\"{text}\" is not a host name; write a full name, like blog.example.com"
        ));
    }
    let named = &labels[usize::from(wildcard)..];
    if name.len() > MAX_HOST_LEN || !named.iter().all(|l| good_label(l)) {
        return Err(format!(
            "\"{text}\" is not a host name; use letters, digits and - between dots, like blog.example.com"
        ));
    }
    if labels
        .last()
        .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!(
            "\"{text}\" is not a host name or an IP address; write a name like blog.example.com or an address like 203.0.113.10"
        ));
    }
    Ok(name)
}

fn good_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= MAX_LABEL_LEN
        && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !l.starts_with('-')
        && !l.ends_with('-')
}

pub fn is_wildcard(host: &str) -> bool {
    host.starts_with("*.")
}

pub fn is_ip(host: &str) -> bool {
    host.parse::<IpAddr>().is_ok()
}

pub fn covers(pattern: &str, name: &str) -> bool {
    if pattern == name {
        return true;
    }
    match (pattern.strip_prefix("*."), name.split_once('.')) {
        (Some(parent), Some((first, rest))) => !first.is_empty() && first != "*" && rest == parent,
        _ => false,
    }
}

pub fn wildcard_of(name: &str) -> Option<String> {
    let (first, rest) = name.split_once('.')?;
    (!first.is_empty() && rest.contains('.')).then(|| format!("*.{rest}"))
}

pub fn sni_name(raw: &str) -> String {
    raw.strip_suffix('.').unwrap_or(raw).to_ascii_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Twice {
    pub first: usize,
    pub second: usize,
}

pub fn entry_for<'a>(
    entries: impl IntoIterator<Item = (usize, &'a [String])>,
    name: &str,
) -> Result<Option<usize>, Twice> {
    let name = sni_name(name.trim());
    let (mut exact, mut wild): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    for (i, hosts) in entries {
        let listed = hosts.iter().map(|h| sni_name(h.trim()));
        let mut kind = None;
        for h in listed {
            if h == name {
                kind = Some(true);
                break;
            }
            if covers(&h, &name) {
                kind = Some(false);
            }
        }
        match kind {
            Some(true) => exact.push(i),
            Some(false) => wild.push(i),
            None => {}
        }
    }
    let pick = if exact.is_empty() { wild } else { exact };
    match pick.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        [first, second, ..] => Err(Twice {
            first: *first,
            second: *second,
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Dns,
    Tls,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCheck {
    DnsNotSet,
    DnsForIp,
    NoPort,
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
    challenge_dns: bool,
) -> Result<Vec<Check>, NoCheck> {
    let name = sni_name(name.trim());
    if is_wildcard(&name) || challenge_dns {
        return match (is_ip(&name), dns) {
            (true, _) => Err(NoCheck::DnsForIp),
            (false, false) => Err(NoCheck::DnsNotSet),
            (false, true) => Ok(vec![Check::Dns]),
        };
    }
    let mut out = Vec::new();
    if ports.https == Some(TLS_PORT) {
        out.push(Check::Tls);
    }
    if ports.http == Some(HTTP_PORT) {
        out.push(Check::Http);
    }
    if out.is_empty() {
        return Err(NoCheck::NoPort);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: SingleLabel = SingleLabel::Refused;

    fn ports(https: Option<u16>, http: Option<u16>) -> CheckPorts {
        CheckPorts { https, http }
    }

    #[test]
    fn a_name_is_checked_over_dns_for_a_wildcard_else_over_tls_or_http() {
        let both = ports(Some(443), Some(80));
        assert_eq!(
            checks_for("blog.example.com", both, false, false),
            Ok(vec![Check::Tls, Check::Http])
        );
        assert_eq!(
            checks_for(
                "blog.example.com",
                ports(Some(8443), Some(80)),
                false,
                false
            ),
            Ok(vec![Check::Http])
        );
        assert_eq!(
            checks_for("blog.example.com", ports(Some(443), None), false, false),
            Ok(vec![Check::Tls])
        );
        assert_eq!(
            checks_for(
                "blog.example.com",
                ports(Some(8443), Some(8080)),
                false,
                false
            ),
            Err(NoCheck::NoPort)
        );
        assert_eq!(
            checks_for("*.apps.example.com", both, false, false),
            Err(NoCheck::DnsNotSet)
        );
        assert_eq!(
            checks_for("*.apps.example.com", ports(None, None), true, false),
            Ok(vec![Check::Dns])
        );
        assert_eq!(
            checks_for("lab.example.net", both, true, true),
            Ok(vec![Check::Dns])
        );
        assert_eq!(
            checks_for("lab.example.net", both, false, true),
            Err(NoCheck::DnsNotSet)
        );
        assert_eq!(
            checks_for("203.0.113.10", both, true, true),
            Err(NoCheck::DnsForIp)
        );
        assert_eq!(
            checks_for("203.0.113.10", both, false, false),
            Ok(vec![Check::Tls, Check::Http])
        );
    }

    fn entries(list: &[&[&str]]) -> Vec<Vec<String>> {
        list.iter()
            .map(|e| e.iter().map(|h| h.to_string()).collect())
            .collect()
    }

    fn pick(list: &[&[&str]], name: &str) -> Result<Option<usize>, Twice> {
        let e = entries(list);
        entry_for(e.iter().enumerate().map(|(i, h)| (i, h.as_slice())), name)
    }

    #[test]
    fn an_exact_listing_wins_over_a_wildcard_listing() {
        let list: &[&[&str]] = &[&["*.example.com"], &["Blog.Example.com."]];
        assert_eq!(pick(list, "blog.example.com"), Ok(Some(1)));
        assert_eq!(pick(list, "app.example.com"), Ok(Some(0)));
        assert_eq!(pick(list, "BLOG.example.com."), Ok(Some(1)));
    }

    #[test]
    fn a_wildcard_covers_exactly_one_leftmost_label() {
        let list: &[&[&str]] = &[&["*.example.com"]];
        assert_eq!(pick(list, "app.example.com"), Ok(Some(0)));
        assert_eq!(pick(list, "example.com"), Ok(None));
        assert_eq!(pick(list, "a.b.example.com"), Ok(None));
        assert_eq!(pick(list, "*.example.com"), Ok(Some(0)));
        assert_eq!(pick(list, "other.org"), Ok(None));
    }

    #[test]
    fn a_name_listed_in_two_entries_is_an_error() {
        let list: &[&[&str]] = &[&["a.example.com"], &["b.example.com", "a.example.com"]];
        assert_eq!(
            pick(list, "a.example.com"),
            Err(Twice {
                first: 0,
                second: 1
            })
        );
        let wild: &[&[&str]] = &[&["*.example.com"], &["*.example.com"]];
        assert!(pick(wild, "x.example.com").is_err());
        assert_eq!(pick(list, "b.example.com"), Ok(Some(1)));
    }

    #[test]
    fn host_names_are_kept_lower_case_without_the_final_dot() {
        assert_eq!(
            host_name("Blog.Example.COM.", R).unwrap(),
            "blog.example.com"
        );
        assert_eq!(host_name(" nas.home.arpa ", R).unwrap(), "nas.home.arpa");
        assert_eq!(
            host_name("*.Apps.example.com", R).unwrap(),
            "*.apps.example.com"
        );
        assert_eq!(
            host_name("xn--bcher-kva.example", R).unwrap(),
            "xn--bcher-kva.example"
        );
    }

    #[test]
    fn ip_addresses_are_sites_too() {
        assert_eq!(host_name("203.0.113.10", R).unwrap(), "203.0.113.10");
        assert_eq!(host_name("2001:DB8::1", R).unwrap(), "2001:db8::1");
        assert_eq!(host_name("[2001:db8::1]", R).unwrap(), "2001:db8::1");
        assert_eq!(host_name("::ffff:192.0.2.1", R).unwrap(), "192.0.2.1");
        assert!(is_ip("192.0.2.1") && !is_ip("example.com"));
    }

    #[test]
    fn the_messages_say_how_to_write_a_name() {
        let e = host_name("https://blog.example.com", R).unwrap_err();
        assert_eq!(
            e,
            "\"https://blog.example.com\" is not a host name; write the name alone, like blog.example.com"
        );
        assert!(
            host_name("blog.example.com:443", R)
                .unwrap_err()
                .contains("the name alone")
        );
        assert_eq!(
            host_name("*.example.*", R).unwrap_err(),
            "\"*.example.*\" is not a valid wildcard; a wildcard is \"*.\" followed by a name, like *.example.com"
        );
        assert!(
            host_name("a*.example.com", R)
                .unwrap_err()
                .contains("not a valid wildcard")
        );
        assert_eq!(
            host_name("*.com", R).unwrap_err(),
            "\"*.com\" is too wide; a wildcard needs a parent name, like *.example.com"
        );
        assert!(host_name("nas", R).unwrap_err().contains("a full name"));
        assert!(host_name("", R).unwrap_err().contains("required"));
        assert!(
            host_name("bad_name.example.com", R)
                .unwrap_err()
                .contains("letters, digits")
        );
        assert!(host_name("-a.example.com", R).is_err());
        assert!(host_name("a..example.com", R).is_err());
        assert!(host_name(&format!("{}.com", "a".repeat(64)), R).is_err());
        assert!(host_name(&format!("{}.com", "a".repeat(63)), R).is_ok());
        assert!(
            host_name("203.0.113", R)
                .unwrap_err()
                .contains("IP address")
        );
    }

    #[test]
    fn a_single_label_name_is_a_host_name_only_when_the_caller_allows_it() {
        let a = SingleLabel::Allowed;
        assert_eq!(host_name("NAS", a).unwrap(), "nas");
        assert_eq!(host_name("localhost.", a).unwrap(), "localhost");
        assert!(host_name("nas", R).unwrap_err().contains("a full name"));
        assert!(host_name("*.nas", a).unwrap_err().contains("too wide"));
        assert!(host_name("123", a).unwrap_err().contains("IP address"));
        assert!(
            host_name("-nas", a)
                .unwrap_err()
                .contains("letters, digits")
        );
    }

    #[test]
    fn a_wildcard_covers_exactly_one_label_as_rfc_6125_says() {
        assert!(covers("*.example.com", "a.example.com"));
        assert!(!covers("*.example.com", "a.b.example.com"));
        assert!(!covers("*.example.com", "example.com"));
        assert!(!covers("*.example.com", "*.example.com.x"));
        assert!(covers("example.com", "example.com"));
        assert!(!covers("a.example.com", "b.example.com"));
        assert_eq!(
            wildcard_of("a.example.com").as_deref(),
            Some("*.example.com")
        );
        assert_eq!(wildcard_of("example.com"), None);
        assert_eq!(wildcard_of("a"), None);
    }

    #[test]
    fn a_sent_name_is_matched_without_case_or_final_dot() {
        assert_eq!(sni_name("NAS.Example.com."), "nas.example.com");
        assert_eq!(sni_name("nas.example.com"), "nas.example.com");
    }
}
