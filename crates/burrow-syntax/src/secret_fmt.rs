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

pub fn url_with_password(text: &str) -> bool {
    let Some((scheme, rest)) = text.trim().split_once("://") else {
        return false;
    };
    let plain = !scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let userinfo = authority.rsplit_once('@').map(|(u, _)| u);
    plain && userinfo.is_some_and(|u| u.split_once(':').is_some_and(|(_, p)| !p.is_empty()))
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
    fn a_url_with_a_password_is_known() {
        for yes in [
            "wss://ops:pw@tunnel.example.com/burrow",
            "https://u:p@ca.internal/acme",
            "socks5://a:b@h:1080",
        ] {
            assert!(url_with_password(yes), "{yes}");
        }
        for no in [
            "wss://tunnel.example.com/burrow",
            "https://user@host/",
            "https://host/path?x=a:b@c",
            "ops@example.com",
            "mailto:a:b@c",
            "https://u:@host/",
        ] {
            assert!(!url_with_password(no), "{no}");
        }
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
