pub fn server_is_older(server: &str, ui: &str) -> bool {
    match (
        semver::Version::parse(server.trim().trim_start_matches('v')),
        semver::Version::parse(ui.trim().trim_start_matches('v')),
    ) {
        (Ok(s), Ok(u)) => s < u,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_parsed_older_server_counts_as_older() {
        assert!(server_is_older("0.1.0", "0.1.1"));
        assert!(server_is_older("v0.1.0", "0.2.0"));
        assert!(!server_is_older("0.1.1", "0.1.1"));
        assert!(!server_is_older("0.2.0", "0.1.1"));
        assert!(server_is_older("0.1.1-rc.1", "0.1.1"));
        assert!(!server_is_older("0.1.1", "0.1.1-rc.1"));
        assert!(!server_is_older("garbage", "0.1.1"));
        assert!(!server_is_older("0.1.0", ""));
    }
}
