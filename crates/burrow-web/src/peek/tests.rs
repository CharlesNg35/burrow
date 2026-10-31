use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::ReadBuf;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, ClientConnection};

use super::*;

pub(crate) fn client_hello(name: &str, alpn: &[&[u8]]) -> Vec<u8> {
    let provider = crate::tls::provider();
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(crate::http::upstream::target_tls::any_certificate())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let server = ServerName::try_from(name.to_string()).unwrap();
    let mut conn = ClientConnection::new(Arc::new(config), server).unwrap();
    let mut out = Vec::new();
    while conn.wants_write() {
        conn.write_tls(&mut out).unwrap();
    }
    out
}

fn split_into_two_records(hello: &[u8]) -> Vec<u8> {
    let body = &hello[5..];
    let (a, b) = body.split_at(body.len() / 2);
    let mut out = Vec::new();
    for part in [a, b] {
        out.extend_from_slice(&hello[..3]);
        out.extend_from_slice(&(part.len() as u16).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

struct Trickle(Vec<u8>, usize);

impl AsyncRead for Trickle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.1 < self.0.len() {
            let b = self.0[self.1];
            buf.put_slice(&[b]);
            self.1 += 1;
        }
        Poll::Ready(Ok(()))
    }
}

fn hello_of(peek: Peek) -> Hello {
    match peek {
        Peek::Hello(h) => h,
        other => panic!("expected a hello, got {other:?}"),
    }
}

#[tokio::test]
async fn the_name_and_alpn_are_read_and_every_byte_is_kept() {
    let sent = client_hello("NAS.Example.com", &[b"h2", b"http/1.1"]);
    let mut from = &sent[..];
    let h = hello_of(peek_sni(&mut from, Duration::from_secs(5)).await.unwrap());
    assert_eq!(h.name.as_deref(), Some("nas.example.com"));
    assert_eq!(h.alpn, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
    assert_eq!(h.bytes, sent);
}

#[tokio::test]
async fn a_hello_fed_one_byte_per_read_is_parsed_and_returned_unchanged() {
    let sent = client_hello("a.example.com", &[]);
    let mut slow = Trickle(sent.clone(), 0);
    let h = hello_of(peek_sni(&mut slow, Duration::from_secs(5)).await.unwrap());
    assert_eq!(h.name.as_deref(), Some("a.example.com"));
    assert!(h.alpn.is_empty());
    assert_eq!(h.bytes, sent);
}

#[tokio::test]
async fn a_hello_split_over_two_records_is_read_whole() {
    let sent = split_into_two_records(&client_hello("b.example.com", &[]));
    let mut from = &sent[..];
    let h = hello_of(peek_sni(&mut from, Duration::from_secs(5)).await.unwrap());
    assert_eq!(h.name.as_deref(), Some("b.example.com"));
    assert_eq!(h.bytes, sent);
}

#[test]
fn a_hello_without_a_name_has_none() {
    let mut p = Peeker::default();
    let sent = client_hello("192.0.2.7", &[]);
    let h = hello_of(p.feed(&sent).unwrap());
    assert_eq!(h.name, None);
}

#[test]
fn plain_http_and_other_bytes_are_not_tls() {
    assert_eq!(
        Peeker::default().feed(b"GET / HTTP/1.1\r\n"),
        Some(Peek::NotTls)
    );
    assert_eq!(
        Peeker::default().feed(&[0x16, 0x03, 0x01, 0x00, 0x04, 0xff, 0, 0, 0]),
        Some(Peek::NotTls)
    );
    let mut p = Peeker::default();
    assert_eq!(p.feed(&[HANDSHAKE_RECORD]), None);
    assert_eq!(p.feed(&[]), Some(Peek::Closed));
}

#[test]
fn more_than_one_full_record_without_a_hello_is_too_big() {
    let mut p = Peeker::default();
    let mut header = vec![0x16, 0x03, 0x01, 0x40, 0x00, 0x01, 0x00, 0xff, 0xff];
    header.resize(MAX_HELLO - 1, 0);
    assert_eq!(p.feed(&header), None);
    assert_eq!(p.feed(&[0, 0]), Some(Peek::TooBig));
}

#[tokio::test]
async fn a_silent_connection_times_out() {
    let (mut a, _b) = tokio::io::duplex(64);
    let e = peek_sni(&mut a, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
}
