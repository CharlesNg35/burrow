use std::net::SocketAddr;

use burrow_config::Bind;

pub use burrow_syntax::ports::{
    MAX_RANGE, parse_range, port_at, ranges_conflict, show, show_addr, split_target,
};

pub fn parse_ranged(
    bind: &Bind,
    default_ip: [u8; 4],
    what: &str,
) -> Result<(SocketAddr, u16), String> {
    bind.ranged(default_ip, what)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ranges_parse_and_print() {
        assert_eq!(parse_range("30000-30010").unwrap(), (30000, 11));
        assert_eq!(parse_range(" 22 ").unwrap(), (22, 1));
        assert_eq!(parse_range("5-5").unwrap(), (5, 1));
        assert!(
            parse_range("10-9")
                .unwrap_err()
                .contains("ends before it starts")
        );
        assert!(parse_range("0-5").is_err());
        assert!(parse_range("1-2000").unwrap_err().contains("limit is 1000"));
        assert!(parse_range("a-b").is_err());
        assert_eq!(show(30000, 11), "30000-30010");
        assert_eq!(show(22, 1), "22");
        assert_eq!(show_addr(sa("[::1]:80"), 3), "[::1]:80-82");
        assert_eq!(show_addr(sa("0.0.0.0:80"), 1), "0.0.0.0:80");
    }

    #[test]
    fn binds_with_ranges() {
        let p = |b: Bind| parse_ranged(&b, [0, 0, 0, 0], "bind");
        assert_eq!(p(Bind::Port(80)).unwrap(), (sa("0.0.0.0:80"), 1));
        assert_eq!(
            p(Bind::Addr("30000-30010".into())).unwrap(),
            (sa("0.0.0.0:30000"), 11)
        );
        assert_eq!(
            p(Bind::Addr("127.0.0.1:30000-30002".into())).unwrap(),
            (sa("127.0.0.1:30000"), 3)
        );
        assert_eq!(
            p(Bind::Addr("[::]:5000-5001".into())).unwrap(),
            (sa("[::]:5000"), 2)
        );
        assert_eq!(
            p(Bind::Addr("[::1]:22".into())).unwrap(),
            (sa("[::1]:22"), 1)
        );
        assert!(p(Bind::Addr("::1:22".into())).is_err());
        assert!(p(Bind::Addr("localhost:22".into())).is_err());
        assert!(p(Bind::Port(0)).is_err());
        assert!(p(Bind::Addr("0.0.0.0:0".into())).is_err());
    }

    #[test]
    fn malformed_brackets_and_edge_ranges() {
        for bad in ["[::1:80", "[::1]]:80", "[127.0.0.1]:80", "::1:80"] {
            assert!(
                parse_ranged(&Bind::Addr(bad.into()), [0, 0, 0, 0], "bind").is_err(),
                "{bad} was accepted"
            );
        }
        assert_eq!(show(65535, 2), "65535-65535");
        assert_eq!(show(64536, 1000), "64536-65535");
    }

    #[test]
    fn targets_with_ranges() {
        assert_eq!(
            split_target("127.0.0.1:30000-30010").unwrap(),
            ("127.0.0.1:30000".into(), 11)
        );
        assert_eq!(
            split_target("db.local:5432").unwrap(),
            ("db.local:5432".into(), 1)
        );
        assert_eq!(split_target("[::1]:80-81").unwrap(), ("[::1]:80".into(), 2));
        assert!(split_target("h:9-1").is_err());
        assert_eq!(port_at("127.0.0.1:30000", 5), "127.0.0.1:30005");
        assert_eq!(port_at("[::1]:80", 1), "[::1]:81");
        assert_eq!(port_at("db:5432", 0), "db:5432");
    }

    #[test]
    fn overlapping_ranges_conflict() {
        assert!(ranges_conflict(
            sa("0.0.0.0:100"),
            10,
            sa("127.0.0.1:105"),
            1
        ));
        assert!(ranges_conflict(sa("0.0.0.0:100"), 10, sa("0.0.0.0:95"), 6));
        assert!(!ranges_conflict(
            sa("0.0.0.0:100"),
            10,
            sa("0.0.0.0:110"),
            5
        ));
        assert!(!ranges_conflict(
            sa("10.0.0.1:100"),
            10,
            sa("10.0.0.2:100"),
            10
        ));
        assert!(ranges_conflict(sa("0.0.0.0:80"), 1, sa("[::]:80"), 1));
    }
}
