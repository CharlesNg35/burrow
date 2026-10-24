#![no_main]

use burrow::visitor::socks_udp::parse;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(h) = parse(data) {
        assert_eq!(h.head.len() + h.data.len(), data.len());
        assert_eq!(h.head[2], 0);
        let again = parse(h.head).expect("a header that parsed parses alone");
        assert_eq!(again.target, h.target);
        assert!(again.data.is_empty());
        assert!(h.target.len() <= 262);
    }
});
