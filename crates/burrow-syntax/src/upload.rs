#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pair,
    Ca,
    Crl,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Pair, Kind::Ca, Kind::Crl];

    pub fn prefix(self) -> &'static str {
        match self {
            Kind::Pair => "pair",
            Kind::Ca => "ca",
            Kind::Crl => "crl",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.prefix() == text)
    }

    pub fn what(self) -> &'static str {
        match self {
            Kind::Pair => "a certificate and its key",
            Kind::Ca => "a CA file",
            Kind::Crl => "a revocation list",
        }
    }
}

pub fn kind_of(id: &str) -> Option<Kind> {
    let (kind, hash) = id.split_once('-')?;
    let hex = hash.len() == 16
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    Kind::parse(kind).filter(|_| hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_kind_and_sixteen_lowercase_hex_digits() {
        assert_eq!(kind_of("pair-0123456789abcdef"), Some(Kind::Pair));
        assert_eq!(kind_of("ca-0123456789abcdef"), Some(Kind::Ca));
        assert_eq!(kind_of("crl-0123456789abcdef"), Some(Kind::Crl));
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
