#![no_main]

use burrow_web::proxy_header::{MAX_HEADER, Parsed, parse};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| match parse(data) {
    Parsed::Header { len, source } => {
        assert!(len <= MAX_HEADER && len <= data.len());
        assert_eq!(
            parse(&data[..len]),
            Parsed::Header { len, source },
            "the header alone parses the same"
        );
    }
    Parsed::More => assert!(data.len() < MAX_HEADER),
    Parsed::Invalid => {}
});
