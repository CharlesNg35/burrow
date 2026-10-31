pub const SECRET_WORD: &str = "secret";
pub const SECRET_LIKE: [&str; 6] = ["key", "psk", "token", "password", "pass", "passwd"];

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
    fn shapes() {
        assert!(looks_like_secret(&"ab".repeat(32)));
        assert!(looks_like_secret(
            "q2b7oVJx0qkZ0x8eF1pS7uR6m9cT3dW4yL5nH2gK8aE="
        ));
        assert!(!looks_like_secret("password"));
    }
}
