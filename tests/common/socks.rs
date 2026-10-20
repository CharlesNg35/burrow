use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub fn connect_request(host: &str, port: u16) -> Vec<u8> {
    let mut req = vec![5, 1, 0];
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        req.push(1);
        req.extend_from_slice(&ip.octets());
    } else if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        req.push(4);
        req.extend_from_slice(&ip.octets());
    } else {
        req.push(3);
        req.push(host.len() as u8);
        req.extend_from_slice(host.as_bytes());
    }
    req.extend_from_slice(&port.to_be_bytes());
    req
}

pub async fn greet(proxy: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut chosen = [0u8; 2];
    s.read_exact(&mut chosen).await.unwrap();
    assert_eq!(chosen, [5, 0]);
    s
}

pub async fn reply_code(s: &mut TcpStream) -> u8 {
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("a SOCKS reply in time")
        .unwrap();
    assert_eq!(reply[0], 5);
    reply[1]
}

pub async fn socks_connect(proxy: SocketAddr, host: &str, port: u16) -> (u8, TcpStream) {
    let mut s = greet(proxy).await;
    s.write_all(&connect_request(host, port)).await.unwrap();
    let code = reply_code(&mut s).await;
    (code, s)
}

pub async fn echoes(mut s: TcpStream, data: &[u8]) {
    let (mut r, mut w) = s.split();
    let send = async {
        w.write_all(data).await.unwrap();
        w.shutdown().await.unwrap();
    };
    let mut back = Vec::new();
    let (_, got) = tokio::join!(send, r.read_to_end(&mut back));
    got.unwrap();
    assert_eq!(back, data);
}
