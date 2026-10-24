#![no_main]

use burrow_web::peek::{MAX_HELLO, Peek, Peeker};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((size, stream)) = data.split_first() else {
        return;
    };
    let mut peeker = Peeker::default();
    for chunk in stream.chunks(usize::from(*size % 64) + 1) {
        match peeker.feed(chunk) {
            None => {}
            Some(Peek::Hello(h)) => {
                assert!(h.bytes.len() <= MAX_HELLO);
                assert_eq!(
                    h.bytes,
                    stream[..h.bytes.len()],
                    "the hello is returned unchanged"
                );
                if let Some(name) = h.name {
                    assert_eq!(name, name.to_ascii_lowercase());
                    assert!(!name.ends_with('.'));
                }
                return;
            }
            Some(_) => return,
        }
    }
});
