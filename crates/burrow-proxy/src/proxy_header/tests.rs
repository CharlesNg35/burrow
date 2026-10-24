use super::*;

fn v2(command: u8, family: u8, addrs: &[u8]) -> Vec<u8> {
    let mut h = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    h.push(0x20 | command);
    h.push(family);
    h.extend_from_slice(&(addrs.len() as u16).to_be_bytes());
    h.extend_from_slice(addrs);
    h
}

fn ipv4_block() -> Vec<u8> {
    let mut a = vec![198, 51, 100, 7, 10, 0, 0, 1];
    a.extend_from_slice(&40000u16.to_be_bytes());
    a.extend_from_slice(&443u16.to_be_bytes());
    a
}

#[test]
fn the_parsing_table() {
    let src4: SocketAddr = "198.51.100.7:40000".parse().unwrap();
    let src6: SocketAddr = "[2001:db8::7]:40001".parse().unwrap();
    let v1 = b"PROXY TCP4 198.51.100.7 10.0.0.1 40000 443\r\n";
    let v1_6 = b"PROXY TCP6 2001:db8::7 2001:db8::1 40001 443\r\n";
    let unknown = b"PROXY UNKNOWN\r\n";
    let proxied = v2(1, 0x11, &ipv4_block());
    let local = v2(0, 0x00, &[]);
    let udp = v2(1, 0x12, &ipv4_block());
    let cases: Vec<(&str, Vec<u8>, Parsed)> = vec![
        (
            "v1 tcp4",
            v1.to_vec(),
            Parsed::Header {
                len: v1.len(),
                source: Some(src4),
            },
        ),
        (
            "v1 tcp6",
            v1_6.to_vec(),
            Parsed::Header {
                len: v1_6.len(),
                source: Some(src6),
            },
        ),
        (
            "v1 unknown keeps the peer",
            unknown.to_vec(),
            Parsed::Header {
                len: unknown.len(),
                source: None,
            },
        ),
        (
            "v2 proxy",
            proxied.clone(),
            Parsed::Header {
                len: proxied.len(),
                source: Some(src4),
            },
        ),
        (
            "v2 local keeps the peer",
            local.clone(),
            Parsed::Header {
                len: local.len(),
                source: None,
            },
        ),
        (
            "v2 datagram is not a visitor address",
            udp.clone(),
            Parsed::Header {
                len: udp.len(),
                source: None,
            },
        ),
        ("v1 half", v1[..20].to_vec(), Parsed::More),
        ("v2 half", proxied[..20].to_vec(), Parsed::More),
        (
            "tls instead",
            vec![
                0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
            Parsed::Invalid,
        ),
        (
            "http instead",
            b"GET / HTTP/1.1\r\nHost: a\r\n\r\n".to_vec(),
            Parsed::Invalid,
        ),
        (
            "v1 garbage",
            b"PROXY TCP4 nonsense\r\n".to_vec(),
            Parsed::Invalid,
        ),
        (
            "v1 too long",
            [b"PROXY TCP4 ".as_slice(), &[b'1'; 200]].concat(),
            Parsed::Invalid,
        ),
        (
            "v2 over the cap",
            v2(1, 0x11, &vec![0; 600]),
            Parsed::Invalid,
        ),
    ];
    for (what, input, want) in cases {
        assert_eq!(parse(&input), want, "{what}");
    }
}

#[tokio::test]
async fn the_bytes_after_the_header_reach_the_reader_unchanged() {
    let mut input = b"PROXY TCP4 198.51.100.7 10.0.0.1 40000 443\r\n".to_vec();
    input.extend_from_slice(&[0x16, 0x03, 0x01, 9, 9]);
    let mut from = &input[..];
    let r = read_proxy_header(&mut from, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(r.source, Some("198.51.100.7:40000".parse().unwrap()));
    assert_eq!(r.rest, [0x16, 0x03, 0x01, 9, 9]);
}

#[tokio::test]
async fn a_missing_or_slow_header_is_refused() {
    let mut tls: &[u8] = &[
        0x16, 0x03, 0x01, 0x02, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ];
    let e = read_proxy_header(&mut tls, Duration::from_secs(1))
        .await
        .err()
        .unwrap();
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    let mut cut: &[u8] = b"PROXY TCP4 198.51";
    let e = read_proxy_header(&mut cut, Duration::from_secs(1))
        .await
        .err()
        .unwrap();
    assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
    let (mut silent, _keep) = tokio::io::duplex(8);
    let e = read_proxy_header(&mut silent, Duration::from_millis(30))
        .await
        .err()
        .unwrap();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
}
