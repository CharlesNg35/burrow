#![no_main]

use std::sync::Arc;

use burrow_web::peek::{Hello, MAX_HELLO, Peek, Peeker};
use libfuzzer_sys::fuzz_target;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, ClientConnection, RootCertStore, crypto};

fn feed(stream: &[u8], chunk: usize) -> Option<Peek> {
    let mut peeker = Peeker::default();
    for part in stream.chunks(chunk) {
        if let Some(done) = peeker.feed(part) {
            return Some(done);
        }
    }
    None
}

fn holds(h: &Hello, stream: &[u8]) {
    assert!(h.bytes.len() <= MAX_HELLO);
    assert_eq!(
        h.bytes,
        stream[..h.bytes.len()],
        "the hello is returned unchanged"
    );
    if let Some(name) = &h.name {
        assert_eq!(*name, name.to_ascii_lowercase());
        assert!(!name.ends_with('.'));
    }
}

fn raw(stream: &[u8], size: u8) {
    if let Some(Peek::Hello(h)) = feed(stream, usize::from(size % 64) + 1) {
        holds(&h, stream);
    }
}

fn name_from(bytes: &[u8]) -> Option<String> {
    let text: String = bytes
        .iter()
        .map(|b| match b % 40 {
            n @ 0..=25 => char::from(if b & 0x40 == 0 { b'a' + n } else { b'A' + n }),
            n @ 26..=35 => char::from(b'0' + n - 26),
            36 | 37 => '.',
            _ => '-',
        })
        .take(80)
        .collect();
    let name = burrow_syntax::web::host_name(&text).ok()?;
    let plain = !burrow_syntax::web::is_wildcard(&name) && !burrow_syntax::web::is_ip(&name);
    plain.then_some(name)
}

fn alpn_from(bytes: &[u8], many: bool) -> Vec<Vec<u8>> {
    if many {
        let count = 200 + u16::from(bytes.first().copied().unwrap_or(0) % 80);
        return (0..count)
            .map(|i| {
                let mut p = i.to_be_bytes().to_vec();
                p.resize(60, b'h');
                p
            })
            .collect();
    }
    bytes
        .split(|b| *b == b',')
        .filter(|p| !p.is_empty())
        .take(8)
        .map(|p| p.iter().copied().take(40).collect())
        .collect()
}

fn made(mode: u8, rest: &[u8]) {
    let mut parts = rest.splitn(3, |b| *b == 0);
    let (name_part, alpn_part, tail) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    let name = if mode & 2 == 0 {
        name_from(name_part)
    } else {
        None
    };
    let server = match &name {
        Some(n) => {
            ServerName::try_from(n.clone()).expect("a host name the proxy accepts is a DNS name")
        }
        None => ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into()),
    };
    let provider = Arc::new(crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    config.alpn_protocols = alpn_from(alpn_part, mode & 4 != 0);
    let Ok(mut conn) = ClientConnection::new(Arc::new(config), server) else {
        return;
    };
    let mut hello = Vec::new();
    while conn.wants_write() {
        conn.write_tls(&mut hello).expect("write to a vec");
    }
    let chunk = usize::from(tail.first().copied().unwrap_or(0) % 64) + 1;
    let flip = (tail.len() >= 3).then(|| {
        let at = usize::from(u16::from_be_bytes([tail[1], tail[2]])) % hello.len();
        (at, tail[0] | 1)
    });
    if let Some((at, by)) = flip {
        let mut bent = hello.clone();
        bent[at] ^= by;
        raw(&bent, tail[0]);
        return;
    }
    match feed(&hello, chunk) {
        Some(Peek::Hello(h)) => {
            holds(&h, &hello);
            assert_eq!(h.bytes, hello, "the whole hello and nothing more");
            assert_eq!(h.name, name, "the name the client sent");
            assert_eq!(h.alpn, alpn_from(alpn_part, mode & 4 != 0));
        }
        Some(Peek::TooBig) => assert!(hello.len() > MAX_HELLO),
        other => panic!("a real hello of {} bytes read as {other:?}", hello.len()),
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    if mode % 2 == 0 {
        let Some((&size, stream)) = rest.split_first() else {
            return;
        };
        raw(stream, size);
    } else {
        made(mode, rest);
    }
});
