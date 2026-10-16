use std::time::Instant;

const PSK: [u8; 32] = [7u8; 32];
const TOTAL: usize = 1 << 30;

fn pair(pattern: &str) -> (snow::TransportState, snow::TransportState) {
    let params: snow::params::NoiseParams = pattern.parse().unwrap();
    let mut i = snow::Builder::new(params.clone())
        .psk(0, &PSK)
        .unwrap()
        .build_initiator()
        .unwrap();
    let mut r = snow::Builder::new(params)
        .psk(0, &PSK)
        .unwrap()
        .build_responder()
        .unwrap();
    let mut a = [0u8; 256];
    let mut b = [0u8; 256];
    let n = i.write_message(&[], &mut a).unwrap();
    r.read_message(&a[..n], &mut b).unwrap();
    let n = r.write_message(&[], &mut a).unwrap();
    i.read_message(&a[..n], &mut b).unwrap();
    (
        i.into_transport_mode().unwrap(),
        r.into_transport_mode().unwrap(),
    )
}

fn run(pattern: &str, chunk: usize) {
    let (mut tx, mut rx) = pair(pattern);
    let plain = vec![0xabu8; chunk];
    let mut cipher = vec![0u8; chunk + 16];
    let mut out = vec![0u8; chunk];
    let rounds = TOTAL / chunk;
    let start = Instant::now();
    for _ in 0..rounds {
        let n = tx.write_message(&plain, &mut cipher).unwrap();
        rx.read_message(&cipher[..n], &mut out).unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{pattern:<44} chunk={chunk:>6} seal+open {:>7.0} MiB/s",
        (rounds * chunk) as f64 / secs / (1 << 20) as f64
    );
}

fn handshakes(pattern: &str) {
    let n = 2000;
    let start = Instant::now();
    for _ in 0..n {
        pair(pattern);
    }
    let us = start.elapsed().as_secs_f64() * 1e6 / n as f64;
    println!("{pattern:<44} full handshake (both sides) {us:>7.1} us");
}

fn main() {
    for pattern in [
        "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s",
        "Noise_NNpsk0_25519_AESGCM_BLAKE2s",
    ] {
        handshakes(pattern);
        for chunk in [4096, 16384, 32768, 65519] {
            run(pattern, chunk);
        }
    }
}
