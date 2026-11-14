pub const SECRET_WORD: &str = "secret";
pub const SECRET_LIKE: [&str; 6] = ["key", "psk", "token", "password", "pass", "passwd"];

pub const CREDENTIAL_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];
pub const CREDENTIAL_HEADER_ENDINGS: [&str; 5] =
    ["-key", "-token", "-secret", "-password", "-auth"];

pub fn credential_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    CREDENTIAL_HEADERS.contains(&name.as_str())
        || CREDENTIAL_HEADER_ENDINGS.iter().any(|e| name.ends_with(e))
}

pub const URL_USERINFO_SHOWN: &str = "«hidden»";

fn scheme_before(text: &str) -> bool {
    text.bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        .any(|b| b.is_ascii_alphabetic())
}

fn scheme_start(text: &str) -> usize {
    text.len()
        - text
            .bytes()
            .rev()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
            .count()
}

fn url_end(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '`')
}

fn userinfo(text: &str, start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let sign = text[start..end].rfind('@')?;
    let (user, password) = text[start..start + sign].split_once(':')?;
    (!password.is_empty() && !user.contains(['/', '?', '#', '[', ']']))
        .then_some(start..start + sign)
}

pub fn url_passwords(text: &str) -> Vec<std::ops::Range<usize>> {
    let marks: Vec<usize> = text.match_indices("://").map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (n, &at) in marks.iter().enumerate() {
        let start = at + 3;
        let limit = marks.get(n + 1).copied().unwrap_or(text.len());
        if !scheme_before(&text[..at]) || limit < start {
            continue;
        }
        let quote = text[..scheme_start(&text[..at])]
            .chars()
            .next_back()
            .filter(|c| matches!(c, '"' | '\''));
        let quoted = quote.and_then(|q| text[start..limit].rfind(q));
        let end = match quoted {
            Some(i) => start + i,
            None => text[start..limit]
                .find(url_end)
                .map_or(limit, |i| start + i),
        };
        out.extend(userinfo(text, start, end));
    }
    out
}

pub fn value_url_passwords(value: &str) -> Vec<std::ops::Range<usize>> {
    let whole = value
        .find("://")
        .filter(|&at| scheme_before(&value[..at]))
        .and_then(|at| userinfo(value, at + 3, value.len()));
    let mut all: Vec<_> = whole.into_iter().chain(url_passwords(value)).collect();
    all.sort_by_key(|r| r.start);
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for r in all {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

pub fn url_with_password(value: &str) -> bool {
    !value_url_passwords(value).is_empty()
}

fn hide(text: &str, ranges: Vec<std::ops::Range<usize>>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for r in ranges {
        out.push_str(&text[at..r.start]);
        out.push_str(URL_USERINFO_SHOWN);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}

pub fn hide_url_passwords(text: &str) -> String {
    hide(text, url_passwords(text))
}

pub fn hide_value_url_passwords(value: &str) -> String {
    hide(value, value_url_passwords(value))
}

pub const SECRET_LEN: usize = 32;
pub const MAX_SECRETS: usize = 4;

pub fn looks_like_secret(text: &str) -> bool {
    let t = text.trim();
    let hex = t.len() == SECRET_LEN * 2 && t.bytes().all(|b| b.is_ascii_hexdigit());
    let b64 = matches!(t.len(), 43 | 44)
        && t.trim_end_matches('=')
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_'));
    hex || b64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_part_of_a_url_is_hidden_wherever_it_stands() {
        let h = URL_USERINFO_SHOWN;
        for (text, shown) in [
            (
                "wss://ops:pw@tunnel.example.com/burrow",
                format!("wss://{h}@tunnel.example.com/burrow"),
            ),
            (
                "https://acme:literalpw22@${CA_HOST}/dir",
                format!("https://{h}@${{CA_HOST}}/dir"),
            ),
            (
                "https://acme:${ACME_PW}@ca.internal/dir",
                format!("https://{h}@ca.internal/dir"),
            ),
            (
                "https://${U}:${P}@ca.internal",
                format!("https://{h}@ca.internal"),
            ),
            (
                "https://acme:pa/ss55word@ca.internal/dir",
                format!("https://{h}@ca.internal/dir"),
            ),
            (
                "https://acme:p?a#s:s@ca.internal/x?y#z",
                format!("https://{h}@ca.internal/x?y#z"),
            ),
            (
                "https://a:b@c@ca.internal",
                format!("https://{h}@ca.internal"),
            ),
            (
                "https://us%40er:p%2Fw%3A@ca.internal",
                format!("https://{h}@ca.internal"),
            ),
            (
                "https://u:p@[2001:db8::1]:8443/dir",
                format!("https://{h}@[2001:db8::1]:8443/dir"),
            ),
            ("socks5://a:b@h:1080", format!("socks5://{h}@h:1080")),
            (
                "http://acme:literalpw44@ca.internal is not https",
                format!("http://{h}@ca.internal is not https"),
            ),
            (
                "see \"https://u:secretpw@h/x\", then",
                format!("see \"https://{h}@h/x\", then"),
            ),
            (
                "# old: https://u:pw1@a.example and ftp://v:pw2@b.example",
                format!("# old: https://{h}@a.example and ftp://{h}@b.example"),
            ),
            (
                "x=https://u:p@h,y=ftp://w:q@k",
                format!("x=https://{h}@h,y=ftp://{h}@k"),
            ),
            ("(https://u:p@h)", format!("(https://{h}@h)")),
            ("1x://u:p@h", format!("1x://{h}@h")),
            (
                "\x1b[2murl\x1b[0m\x1b[2m=\x1b[0mhttps://u:pw@h\x1b[0m",
                format!("\x1b[2murl\x1b[0m\x1b[2m=\x1b[0mhttps://{h}@h\x1b[0m"),
            ),
            ("https://h:8443/@alice", format!("https://{h}@alice")),
        ] {
            assert_eq!(hide_url_passwords(text), shown, "{text}");
            assert!(url_with_password(text), "{text}");
        }
        for same in [
            "wss://tunnel.example.com/burrow",
            "https://user@host/",
            "https://u:@host/",
            "https://host/path?x=a:b@c",
            "https://[2001:db8::1]:8443/a?x=b:c@d",
            "https://${CA_HOST}/dir",
            "ops@example.com",
            "mailto:a:b@c",
            "user:pass@host",
            "://u:p@h",
            "1://u:p@h",
            "a plain sentence: with a colon @ and an at",
            "",
        ] {
            assert_eq!(hide_url_passwords(same), same, "{same}");
            assert!(!url_with_password(same), "{same}");
        }
    }

    #[test]
    fn a_whole_value_hides_up_to_its_last_at_and_quoted_text_up_to_its_quote() {
        let h = URL_USERINFO_SHOWN;
        for c in [' ', '<', '>', '"', '`', '\t'] {
            let value = format!("socks5://ops:pa{c}ss@proxy.local:1080");
            let shown = format!("socks5://{h}@proxy.local:1080");
            assert_eq!(hide_value_url_passwords(&value), shown, "{value:?}");
            assert!(url_with_password(&value), "{value:?}");
            let quoted = format!("\"{value}\" is not production, staging or an https:// URL");
            let hidden = hide_url_passwords(&quoted);
            assert!(!hidden.contains("ss@"), "{hidden:?}");
        }
        assert_eq!(
            hide_value_url_passwords("x=https://u:p@h,y=ftp://w:q@k"),
            format!("x=https://{h}@k")
        );
        assert_eq!(
            hide_value_url_passwords("https://ca.internal/dir"),
            "https://ca.internal/dir"
        );
    }

    #[test]
    fn shapes() {
        assert!(looks_like_secret(&"ab".repeat(32)));
        assert!(looks_like_secret(
            "q2b7oVJx0qkZ0x8eF1pS7uR6m9cT3dW4yL5nH2gK8aE="
        ));
        assert!(!looks_like_secret("password"));
    }

    #[test]
    fn credential_headers_are_known_by_name() {
        for name in [
            "Authorization",
            "proxy-authorization",
            "COOKIE",
            "Set-Cookie",
            "X-Api-Key",
            "X-Auth-Token",
            "X-Client-Secret",
            "X-Db-Password",
            "X-Upstream-Auth",
        ] {
            assert!(credential_header(name), "{name}");
        }
        for name in [
            "Host",
            "X-Frame-Options",
            "Key",
            "X-Keyring",
            "X-Author",
            "Cookie-Policy",
        ] {
            assert!(!credential_header(name), "{name}");
        }
    }
}
