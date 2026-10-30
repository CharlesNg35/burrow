use blake2::Digest;
use burrow_admin_proto::REV_PREFIX;

use crate::wire::secret::Secret;

const REV_BYTES: usize = 8;

fn rev_of(digest: &[u8]) -> String {
    format!("{REV_PREFIX}{}", hex::encode(&digest[..REV_BYTES]))
}

pub fn rev_hex(rev: &str) -> Option<&str> {
    rev.strip_prefix(REV_PREFIX)
}

pub fn rev_from_hex(hex: &str) -> Option<String> {
    let good = hex.len() == 2 * REV_BYTES && hex.bytes().all(|b| b.is_ascii_hexdigit());
    good.then(|| format!("{REV_PREFIX}{hex}"))
}

pub fn rev(text: &str) -> String {
    rev_of(&blake2::Blake2s256::digest(text.as_bytes()))
}

pub fn hash_parts(parts: &[&[u8]]) -> String {
    let mut h = blake2::Blake2s256::new();
    for p in parts {
        h.update((p.len() as u64).to_be_bytes());
        h.update(p);
    }
    rev_of(&h.finalize())
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')
}

pub fn holds_secret(text: &str) -> bool {
    text.split(|c: char| !token_char(c))
        .any(|t| !t.is_empty() && Secret::parse(t).is_ok())
}

pub fn mask_secrets(line: &str, fresh: &[(String, String)]) -> String {
    let mut out = String::with_capacity(line.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        if let Some((_, fp)) = fresh.iter().find(|(v, _)| v == token) {
            out.push_str(&format!("«new secret {fp}»"));
        } else if let Ok(s) = Secret::parse(token) {
            out.push_str(&format!("«secret {}»", s.fingerprint()));
        } else if token.len() >= 24 && looks_random(token) {
            out.push_str(&"*".repeat(token.len()));
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for c in line.chars() {
        if token_char(c) {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
            out.push(c);
        }
    }
    flush(&mut token, &mut out);
    out
}

fn looks_random(t: &str) -> bool {
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let upper = t.chars().filter(char::is_ascii_uppercase).count();
    let lower = t.chars().filter(char::is_ascii_lowercase).count();
    digits > 0 && upper > 0 && lower > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_replaced_by_fingerprints() {
        let s = Secret::generate();
        let line = format!("    secret: {} # home", s.encode());
        let masked = mask_secrets(&line, &[]);
        assert_eq!(
            masked,
            format!("    secret: «secret {}» # home", s.fingerprint())
        );
        let fresh = vec![(s.encode(), "77aa01ff".to_string())];
        assert!(mask_secrets(&line, &fresh).contains("«new secret 77aa01ff»"));
        assert_eq!(mask_secrets("bind: 7840", &[]), "bind: 7840");
        assert_eq!(rev("a"), rev("a"));
        assert_ne!(rev("a"), rev("b"));
        assert!(rev("a").starts_with("b2s:"));
    }
}
