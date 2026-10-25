#![no_main]

use burrow_web::auth::basic::{encode, parse};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(c) = parse(data) {
        assert!(!c.user.contains(':'));
        assert!(std::str::from_utf8(&c.password).is_ok());
        let again = parse(encode(&c.user, &c.password).as_bytes()).expect("round trip");
        assert_eq!(again, c);
    }
    if let Some((user, password)) = std::str::from_utf8(data)
        .ok()
        .and_then(|s| s.split_once('\n'))
        .filter(|(u, _)| !u.contains(':'))
    {
        let c = parse(encode(user, password.as_bytes()).as_bytes());
        if encode(user, password.as_bytes()).len() <= burrow_web::auth::basic::MAX_CREDENTIALS {
            let c = c.expect("our own encoding parses");
            assert_eq!(c.user, user);
            assert_eq!(c.password.as_slice(), password.as_bytes());
        }
    }
});
