use super::*;

const R: SingleLabel = SingleLabel::Refused;

fn ports(https: Option<u16>, http: Option<u16>) -> CheckPorts {
    CheckPorts { https, http }
}

#[test]
fn a_name_is_checked_over_dns_for_a_wildcard_else_over_tls_or_http() {
    let both = ports(Some(443), Some(80));
    let auto = |n: &str, p| checks_for(n, p, false, None);
    assert_eq!(
        auto("blog.example.com", both),
        Ok(vec![Check::Tls, Check::Http])
    );
    assert_eq!(
        auto("blog.example.com", ports(Some(8443), Some(80))),
        Ok(vec![Check::Http])
    );
    assert_eq!(
        auto("blog.example.com", ports(Some(443), None)),
        Ok(vec![Check::Tls])
    );
    assert_eq!(
        auto("blog.example.com", ports(Some(8443), Some(8080))),
        Err(NoCheck::NoPort)
    );
    assert_eq!(auto("*.apps.example.com", both), Err(NoCheck::DnsNotSet));
    assert_eq!(
        checks_for("*.apps.example.com", ports(None, None), true, None),
        Ok(vec![Check::Dns])
    );
    assert_eq!(
        auto("203.0.113.10", both),
        Ok(vec![Check::Tls, Check::Http])
    );
}

#[test]
fn an_explicit_challenge_needs_its_port_or_its_provider() {
    let only443 = ports(Some(443), None);
    let pick = |n: &str, c| checks_for(n, only443, false, Some(c));
    assert_eq!(
        pick("blog.example.com", Challenge::Tls),
        Ok(vec![Check::Tls])
    );
    assert_eq!(
        pick("blog.example.com", Challenge::Http),
        Err(NoCheck::NoHttpPort)
    );
    assert_eq!(
        pick("blog.example.com", Challenge::Dns),
        Err(NoCheck::DnsNotSet)
    );
    assert_eq!(pick("203.0.113.10", Challenge::Dns), Err(NoCheck::DnsForIp));
    assert_eq!(
        pick("*.example.com", Challenge::Http),
        Err(NoCheck::WildcardNotDns)
    );
    assert_eq!(
        pick("*.example.com", Challenge::Tls),
        Err(NoCheck::WildcardNotDns)
    );
    assert_eq!(
        checks_for(
            "blog.example.com",
            ports(Some(8443), Some(80)),
            false,
            Some(Challenge::Tls)
        ),
        Err(NoCheck::NoTlsPort)
    );
    assert_eq!(
        checks_for(
            "blog.example.com",
            ports(Some(8443), Some(80)),
            false,
            Some(Challenge::Http)
        ),
        Ok(vec![Check::Http])
    );
    assert_eq!(
        checks_for("lab.example.net", only443, true, Some(Challenge::Dns)),
        Ok(vec![Check::Dns])
    );
}

#[test]
fn the_proxy_note_lists_only_names_on_the_automatic_tls_check() {
    let both = ports(Some(443), Some(80));
    let n = |label, host, choice| AutoName {
        label,
        host,
        choice,
    };
    let names = [
        n("kiosk", "203.0.113.10", None),
        n("blog", "blog.example.com", None),
        n("blog", "www.example.com", None),
        n("news", "news.example.com", Some(Challenge::Http)),
        n("apps", "*.apps.example.com", None),
        n(
            "the tunnel's name tunnel.example.com",
            "tunnel.example.com",
            None,
        ),
    ];
    assert_eq!(
        proxy_note(&names, both, false).as_deref(),
        Some(
            "a proxy is in front (web.trusted_proxies is set); if it does the HTTPS, like Cloudflare's proxy, Let's Encrypt's TLS check cannot reach burrow: set challenge: http (with http: 80) or challenge: dns on these sites: kiosk, blog, the tunnel's name tunnel.example.com"
        )
    );
    assert_eq!(
        proxy_note(&names, ports(Some(8443), Some(80)), false),
        None,
        "HTTP first: no TLS check"
    );
    assert_eq!(proxy_note(&names[3..5], both, false), None);
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
