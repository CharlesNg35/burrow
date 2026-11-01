use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

pub const BODY: usize = 256 * 1024;
const READ_FOR: Duration = Duration::from_secs(10);
const RESET_WINDOW: Duration = Duration::from_millis(300);

#[derive(Debug)]
pub struct Ended {
    pub answer: String,
    pub broken: Option<io::Error>,
}

pub async fn send_large_body(addr: SocketAddr, head: &str) -> Ended {
    let stream = TcpStream::connect(addr).await.unwrap();
    send_large_body_over(stream, head, |s| s.take_error().unwrap()).await
}

pub async fn send_large_body_over<S>(
    stream: S,
    head: &str,
    socket_error: impl FnOnce(&S) -> Option<io::Error>,
) -> Ended
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut rd, mut wr) = tokio::io::split(stream);
    let mut request = head.as_bytes().to_vec();
    request.resize(request.len() + BODY, b'x');
    let sender = tokio::spawn(async move {
        let _ = wr.write_all(&request).await;
        let _ = wr.flush().await;
        wr
    });
    let mut out = Vec::new();
    let read = tokio::time::timeout(READ_FOR, rd.read_to_end(&mut out)).await;
    let answer = String::from_utf8_lossy(&out).into_owned();
    let wr = sender.await.unwrap();
    let broken = match read {
        Err(_) => Some(io::ErrorKind::TimedOut.into()),
        Ok(Err(e)) => Some(e),
        Ok(Ok(_)) => {
            tokio::time::sleep(RESET_WINDOW).await;
            socket_error(&rd.unsplit(wr))
        }
    };
    Ended { answer, broken }
}
