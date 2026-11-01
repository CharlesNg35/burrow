use http_body_util::BodyExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

pub(super) type Seen = (::http::request::Parts, Vec<u8>);

pub(super) async fn one_request(answer: u16) -> (String, tokio::task::JoinHandle<Seen>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", l.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (stream, _) = l.accept().await.unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = std::sync::Mutex::new(Some(tx));
        let service =
            hyper::service::service_fn(move |req: ::http::Request<hyper::body::Incoming>| {
                let tx = tx.lock().unwrap().take();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = body.collect().await.unwrap().to_bytes().to_vec();
                    if let Some(tx) = tx {
                        let _ = tx.send((parts, body));
                    }
                    let resp = ::http::Response::builder()
                        .status(answer)
                        .body(http_body_util::Full::new(bytes::Bytes::new()))
                        .unwrap();
                    Ok::<_, std::convert::Infallible>(resp)
                }
            });
        let _ = hyper::server::conn::http1::Builder::new()
            .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
            .await;
        rx.await.unwrap()
    });
    (url, task)
}

pub(super) async fn fake_smtp(auth: &'static [u8]) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let (stream, _) = l.accept().await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();
        let mut got = Vec::new();
        w.write_all(b"220 fake ESMTP\r\n").await.unwrap();
        let mut data = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            got.push(line.clone());
            let reply: &[u8] = match line.as_str() {
                "." if data => {
                    data = false;
                    b"250 queued\r\n"
                }
                _ if data => continue,
                l if l.starts_with("EHLO") => b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n",
                l if l.starts_with("AUTH") => auth,
                "DATA" => {
                    data = true;
                    b"354 go\r\n"
                }
                "QUIT" => {
                    w.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                }
                _ => b"250 ok\r\n",
            };
            w.write_all(reply).await.unwrap();
        }
        got
    });
    (port, task)
}
