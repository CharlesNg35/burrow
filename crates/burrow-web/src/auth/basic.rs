use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

pub const MAX_CREDENTIALS: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    pub password: Zeroizing<Vec<u8>>,
}

pub fn parse(value: &[u8]) -> Option<Credentials> {
    if value.len() > MAX_CREDENTIALS {
        return None;
    }
    let (scheme, rest) = value.split_at(value.iter().position(|b| *b == b' ')?);
    if !scheme.eq_ignore_ascii_case(b"basic") {
        return None;
    }
    let token = rest.trim_ascii();
    if token.is_empty() || token.contains(&b' ') {
        return None;
    }
    let decoded = Zeroizing::new(STANDARD.decode(token).ok()?);
    std::str::from_utf8(&decoded).ok()?;
    let colon = decoded.iter().position(|b| *b == b':')?;
    let user = std::str::from_utf8(&decoded[..colon]).ok()?.to_string();
    let password = Zeroizing::new(decoded[colon + 1..].to_vec());
    Some(Credentials { user, password })
}

pub fn encode(user: &str, password: &[u8]) -> String {
    let mut raw = Zeroizing::new(Vec::with_capacity(user.len() + 1 + password.len()));
    raw.extend_from_slice(user.as_bytes());
    raw.push(b':');
    raw.extend_from_slice(password);
    format!("Basic {}", STANDARD.encode(raw.as_slice()))
}

pub fn challenge(realm: &str) -> String {
    let realm: String = realm
        .chars()
        .filter(|c| *c != '"' && *c != '\\' && !c.is_control())
        .collect();
    format!("Basic realm=\"{realm}\", charset=\"UTF-8\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(user: &str, pw: &str) -> Option<Credentials> {
        Some(Credentials {
            user: user.into(),
            password: Zeroizing::new(pw.as_bytes().to_vec()),
        })
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_the_password_may_hold_colons() {
        assert_eq!(
            parse(encode("bob", b"a:b:c").as_bytes()),
            creds("bob", "a:b:c")
        );
        let lower = encode("bob", b"pw").replacen("Basic", "basic", 1);
        assert_eq!(parse(lower.as_bytes()), creds("bob", "pw"));
        assert_eq!(parse(b"BASIC   Ym9iOnB3"), creds("bob", "pw"));
        assert_eq!(parse(encode("", b"").as_bytes()), creds("", ""));
    }

    #[test]
    fn anything_but_base64_of_utf8_user_colon_password_is_refused() {
        assert_eq!(parse(b"Bearer Ym9iOnB3"), None);
        assert_eq!(parse(b"Basic"), None);
        assert_eq!(parse(b"Basic "), None);
        assert_eq!(parse(b"Basic !!!"), None);
        assert_eq!(parse(b"Basic Ym9i"), None);
        assert_eq!(parse(b"Basic Ym9i OnB3"), None);
        let latin1 = STANDARD.encode(b"b\xf6b:pw");
        assert_eq!(parse(format!("Basic {latin1}").as_bytes()), None);
        let long = format!("Basic {}", "A".repeat(MAX_CREDENTIALS));
        assert_eq!(parse(long.as_bytes()), None);
    }

    #[test]
    fn the_challenge_names_the_realm_and_utf8() {
        assert_eq!(
            challenge("grafana"),
            "Basic realm=\"grafana\", charset=\"UTF-8\""
        );
        assert_eq!(
            challenge("a\"b\\c"),
            "Basic realm=\"abc\", charset=\"UTF-8\""
        );
    }
}
