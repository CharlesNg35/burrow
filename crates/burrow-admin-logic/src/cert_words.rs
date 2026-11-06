use burrow_admin_proto::codes::{
    CHALLENGE_DNS, CHALLENGE_PORT, CHALLENGE_UNUSED, CHALLENGE_WILDCARD, DNS_UNCOVERED,
    INTERNAL_NAME, NO_CHECK, WILDCARD_NEEDS_DNS,
};
use burrow_syntax::web::local_ca::{NotLocal, local_ca_refuses};
use burrow_syntax::web::{HTTP_PORT, NoCheck};

const WEB_PROXY: &str = "Settings, Web proxy";
const DNS_CARD: &str = "DNS check providers";
const OWN_CARD: &str = "Your own certificates";
const OTHER_CHECK: &str = "pick another Let's Encrypt check";

pub const CODES: [&str; 8] = [
    NO_CHECK,
    CHALLENGE_UNUSED,
    WILDCARD_NEEDS_DNS,
    CHALLENGE_DNS,
    CHALLENGE_PORT,
    CHALLENGE_WILDCARD,
    DNS_UNCOVERED,
    INTERNAL_NAME,
];

fn wildcard(host: &str) -> bool {
    host.starts_with("*.")
}

pub fn no_check(host: &str, why: NoCheck) -> String {
    match why {
        NoCheck::NoPort => format!(
            "Let's Encrypt cannot check {host}, as it checks only on ports 443 and 80: set the HTTPS port to 443 or the HTTP port to 80 in {WEB_PROXY}, add its domain under {DNS_CARD}, or use your own certificate"
        ),
        NoCheck::DnsNotSet if wildcard(host) => format!(
            "Let's Encrypt gives the wildcard {host} only through a DNS check: add its domain under {DNS_CARD}, or list it under {OWN_CARD}"
        ),
        NoCheck::DnsNotSet => format!(
            "The DNS check for {host} needs a DNS check provider: add one for its domain under {DNS_CARD}, or {OTHER_CHECK}"
        ),
        NoCheck::DnsForIp => format!(
            "{host} is an IP address, which Let's Encrypt checks only over TLS or HTTP: {OTHER_CHECK}"
        ),
        NoCheck::NoHttpPort => format!(
            "The HTTP check needs the web proxy's HTTP port on 80: set it in {WEB_PROXY}, or {OTHER_CHECK}"
        ),
        NoCheck::NoTlsPort => format!(
            "The TLS check needs the web proxy's HTTPS port on 443: set it in {WEB_PROXY}, or {OTHER_CHECK}"
        ),
        NoCheck::WildcardNotDns => format!(
            "{host} is a wildcard, which Let's Encrypt checks only through DNS: pick DNS as its Let's Encrypt check"
        ),
    }
}

fn uncovered(host: &str, domain: &str) -> String {
    if wildcard(host) {
        format!(
            "Let's Encrypt gives the wildcard {host} only through a DNS check, and no DNS check provider lists {domain}: add it under {DNS_CARD}, or list the name under {OWN_CARD}"
        )
    } else {
        format!(
            "The DNS check for {host} needs a DNS check provider for {domain}: add it under {DNS_CARD}, or {OTHER_CHECK}"
        )
    }
}

fn unused(why: &str) -> String {
    let clear = "clear its Let's Encrypt check";
    match why {
        "passthrough" => format!("The app keeps its own certificate, so Let's Encrypt does not check this site: {clear}"),
        "off" => format!("This site has no HTTPS, so Let's Encrypt does not check it: {clear}"),
        "internal" => format!("The local CA signs this site's certificate, so Let's Encrypt does not check it: {clear}"),
        "tunnel-site" => "The tunnel uses the website's certificate for its name: clear the tunnel's Let's Encrypt check and set it on the website".into(),
        "tunnel" => format!("The tunnel's name gets no certificate from Let's Encrypt: {clear}"),
        _ => format!("This site uses its own certificate, so Let's Encrypt does not check it: {clear}"),
    }
}

pub fn local_ca(host: &str) -> Option<String> {
    Some(match local_ca_refuses(host)? {
        NotLocal::PublicAddress => format!(
            "{host} is a public address, and the local CA signs only addresses of your own network, like 192.168.1.5: pick Automatic for it"
        ),
        NotLocal::WideWildcard => format!(
            "{host} is too wide for the local CA: its wildcards are one label under a name of your network, like *.nas.lan"
        ),
        NotLocal::PublicName => {
            let short = host.split('.').next().unwrap_or(host);
            format!(
                "{host} is not a name on your network, and the local CA signs only names under .lan, .home.arpa, .internal, .local or localhost: pick Automatic for a public name, or name it like {short}.lan"
            )
        }
    })
}

pub(crate) fn worded(code: &str, subjects: &[String]) -> Option<String> {
    let host = subjects.first().map(String::as_str)?;
    let second = subjects.get(1).map(String::as_str);
    let why = match code {
        NO_CHECK => NoCheck::NoPort,
        WILDCARD_NEEDS_DNS => NoCheck::DnsNotSet,
        CHALLENGE_DNS if host.parse::<std::net::IpAddr>().is_ok() => NoCheck::DnsForIp,
        CHALLENGE_DNS => NoCheck::DnsNotSet,
        CHALLENGE_PORT if second.and_then(|p| p.parse().ok()) == Some(HTTP_PORT) => {
            NoCheck::NoHttpPort
        }
        CHALLENGE_PORT => NoCheck::NoTlsPort,
        CHALLENGE_WILDCARD => NoCheck::WildcardNotDns,
        DNS_UNCOVERED => return Some(uncovered(host, second.unwrap_or(host))),
        INTERNAL_NAME => return local_ca(host),
        CHALLENGE_UNUSED => return Some(unused(host)),
        _ => return None,
    };
    Some(no_check(host, why))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn certificate_checks_read_as_plain_words_with_the_card_to_fix_them() {
        let all = [
            worded(NO_CHECK, &s(&["blog.example.com"])),
            worded(WILDCARD_NEEDS_DNS, &s(&["*.apps.example.com"])),
            worded(CHALLENGE_DNS, &s(&["intra.example.com"])),
            worded(CHALLENGE_DNS, &s(&["203.0.113.10"])),
            worded(CHALLENGE_PORT, &s(&["blog.example.com", "80"])),
            worded(CHALLENGE_PORT, &s(&["blog.example.com", "443"])),
            worded(CHALLENGE_WILDCARD, &s(&["*.apps.example.com"])),
            worded(
                DNS_UNCOVERED,
                &s(&["*.apps.example.org", "apps.example.org"]),
            ),
            worded(
                DNS_UNCOVERED,
                &s(&["intra.corp.example.net", "corp.example.net"]),
            ),
            worded(INTERNAL_NAME, &s(&["blog.example.com"])),
            worded(INTERNAL_NAME, &s(&["203.0.113.10"])),
            worded(INTERNAL_NAME, &s(&["*.lan"])),
            worded(CHALLENGE_UNUSED, &s(&["files"])),
            worded(CHALLENGE_UNUSED, &s(&["tunnel-site"])),
        ];
        for w in &all {
            let w = w.as_deref().expect("worded");
            for jargon in [
                "web.",
                "letsencrypt.dns",
                "challenge:",
                "tls:",
                "http:",
                "DNS-01",
                "entry",
            ] {
                assert!(!w.contains(jargon), "{jargon} in {w}");
            }
        }
        assert!(all[1].as_deref().unwrap().contains(DNS_CARD));
        assert!(all[4].as_deref().unwrap().contains("HTTP port on 80"));
        assert!(all[5].as_deref().unwrap().contains("HTTPS port on 443"));
        assert!(
            all[7]
                .as_deref()
                .unwrap()
                .contains("apps.example.org: add it under")
        );
        assert!(all[9].as_deref().unwrap().contains("name it like blog.lan"));
        assert!(
            all[12]
                .as_deref()
                .unwrap()
                .contains("uses its own certificate")
        );
        assert_eq!(worded(INTERNAL_NAME, &s(&["nas.lan"])), None);
        assert_eq!(worded(NO_CHECK, &[]), None);
    }
}
