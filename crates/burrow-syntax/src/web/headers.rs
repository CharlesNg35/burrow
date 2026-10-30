pub const HOP_BY_HOP: [&str; 7] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub const NOT_SETTABLE: [&str; 8] = [
    HOP_BY_HOP[0],
    HOP_BY_HOP[1],
    HOP_BY_HOP[2],
    HOP_BY_HOP[3],
    HOP_BY_HOP[4],
    HOP_BY_HOP[5],
    HOP_BY_HOP[6],
    "content-length",
];

pub fn settable(name: &str) -> bool {
    !NOT_SETTABLE.contains(&name.trim().to_ascii_lowercase().as_str())
}

pub fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_name_is_an_rfc_9110_token() {
        for ok in ["Remote-User", "X-Authentik-Uid", "x_custom", "a.b"] {
            assert!(is_name(ok), "{ok}");
        }
        for bad in ["", "Remote User", "Remote-User:", "Ü"] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn every_hop_by_hop_header_and_content_length_are_burrows_own() {
        for name in HOP_BY_HOP {
            assert!(NOT_SETTABLE.contains(&name), "{name}");
            assert!(!settable(&name.to_ascii_uppercase()), "{name}");
        }
        assert!(!settable(" Content-Length "));
        assert!(settable("X-Frame-Options"));
        assert_eq!(NOT_SETTABLE.len(), HOP_BY_HOP.len() + 1);
    }
}
