pub const KINDS: [&str; 3] = ["pair", "ca", "crl"];

pub fn kind_of(id: &str) -> Option<&'static str> {
    let (kind, hash) = id.split_once('-')?;
    let hex = hash.len() == 16
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    KINDS.into_iter().find(|k| *k == kind).filter(|_| hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_kind_and_sixteen_lowercase_hex_digits() {
        assert_eq!(kind_of("pair-0123456789abcdef"), Some("pair"));
        assert_eq!(kind_of("ca-0123456789abcdef"), Some("ca"));
        assert_eq!(kind_of("crl-0123456789abcdef"), Some("crl"));
        for bad in [
            "key-0123456789abcdef",
            "pair-0123456789ABCDEF",
            "pair-0123456789abcde",
            "pair-0123456789abcdef0",
            "pair_0123456789abcdef",
            "../pair-0123456789abcdef",
            "",
        ] {
            assert_eq!(kind_of(bad), None, "{bad}");
        }
    }
}
