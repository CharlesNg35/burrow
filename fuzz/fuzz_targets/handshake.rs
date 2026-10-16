#![no_main]

use burrow::config::model::Cipher;
use burrow::noise::{Initiator, MSG1_LEN, MSG2_LEN, Responder};
use burrow::protocol::{PREAMBLE_FIXED, Preamble};
use burrow::secret::Secret;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = burrow::transport::ws::route(data, "/burrow");
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(e) = burrow::transport::endpoint::Endpoint::parse(text) {
            let _ = burrow::transport::endpoint::Endpoint::parse(&e.to_string()).expect("display round-trips");
        }
    }
    let secret = Secret::from_bytes([7u8; 32]);
    if data.len() >= PREAMBLE_FIXED {
        let head: [u8; PREAMBLE_FIXED] = data[..PREAMBLE_FIXED].try_into().unwrap();
        if let Ok((kind, flags, len)) = Preamble::decode_fixed(&head) {
            let rest = &data[PREAMBLE_FIXED..];
            if rest.len() >= len + MSG1_LEN {
                if let Ok(p) = Preamble::from_parts(kind, flags, &rest[..len]) {
                    let msg1: [u8; MSG1_LEN] = rest[len..len + MSG1_LEN].try_into().unwrap();
                    let _ = Responder::accept(p.cipher, std::slice::from_ref(&secret), &p.prologue(), &msg1);
                }
            }
        }
    }
    if data.len() >= MSG2_LEN {
        let (init, _) = Initiator::start(Cipher::Chachapoly, &secret, b"fuzz");
        let msg2: [u8; MSG2_LEN] = data[..MSG2_LEN].try_into().unwrap();
        let _ = init.finish(&msg2);
    }
});
