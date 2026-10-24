
use super::*;
use crate::wire::noise::{Initiator, Responder};
use crate::wire::record::{RecordReader, RecordWriter};
use crate::wire::secret::Secret;
use burrow_config::Cipher;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

fn channel(stream: TcpStream) -> Channel {
    let s = Secret::generate();
    let (_, m1) = Initiator::start(Cipher::Chachapoly, &s, b"t");
    let r = Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"t", &m1).unwrap();
    let (session, _) = r.reply().unwrap();
    let (sealer, opener) = session.split();
    Channel {
        stream: Conn::Tcp(stream),
        writer: RecordWriter::new(sealer),
        reader: RecordReader::new(opener),
    }
}

fn channels(a: TcpStream, b: TcpStream) -> (Channel, Channel) {
    let s = Secret::generate();
    let (init, m1) = Initiator::start(Cipher::Chachapoly, &s, b"t");
    let r = Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"t", &m1).unwrap();
    let (theirs, m2) = r.reply().unwrap();
    let ours = init.finish(&m2).unwrap();
    let make = |stream, session: crate::wire::noise::Session| {
        let (sealer, opener) = session.split();
        Channel {
            stream: Conn::Tcp(stream),
            writer: RecordWriter::new(sealer),
            reader: RecordReader::new(opener),
        }
    };
    (make(a, ours), make(b, theirs))
}

#[tokio::test]
async fn a_channel_dropped_after_its_end_of_stream_ends_the_target_connection() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, _silent) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, false));
    server.writer.send(&mut server.stream, &[]).await.unwrap();
    drop(server);
    tokio::time::timeout(std::time::Duration::from_secs(5), pump)
        .await
        .expect("the pump ends once the server has closed the channel")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_plaintext_channel_reset_by_the_server_ends_the_target_connection() {
    let (a, b) = tcp_pair().await;
    let (ch, server) = channels(a, b);
    let (target, _silent) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, true));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    server.stream.reset_on_close();
    drop(server);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .is_ok(),
        "the pump ends once the server has reset the channel"
    );
}

#[tokio::test]
async fn a_plaintext_channel_ends_as_soon_as_both_sides_are_done() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, mut app) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, true));
    server.stream.write_all(b"ping").await.unwrap();
    server.stream.shutdown().await.unwrap();
    let mut got = Vec::new();
    app.read_to_end(&mut got).await.unwrap();
    assert_eq!(got, b"ping");
    app.write_all(b"pong").await.unwrap();
    drop(app);
    let mut reply = Vec::new();
    server.stream.read_to_end(&mut reply).await.unwrap();
    assert_eq!(reply, b"pong");
    let started = std::time::Instant::now();
    tokio::time::timeout(std::time::Duration::from_secs(5), pump)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
}

async fn sees_reset(stream: &mut Conn) -> bool {
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(n) if n > 0 => continue,
                other => return other,
            }
        }
    })
    .await
    .expect("the channel ends");
    matches!(read, Err(e) if e.kind() == io::ErrorKind::ConnectionReset)
}

#[tokio::test]
async fn a_plaintext_channel_whose_target_resets_is_reset_for_the_server() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, app) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, true));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    socket2::SockRef::from(&app)
        .set_linger(Some(std::time::Duration::ZERO))
        .unwrap();
    drop(app);
    assert!(sees_reset(&mut server.stream).await);
    let _ = pump.await;
}

#[tokio::test]
async fn a_cancelled_plaintext_pump_resets_the_channel() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, _silent) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, true));
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    pump.abort();
    assert!(sees_reset(&mut server.stream).await);
}

#[tokio::test]
async fn a_plaintext_channel_that_ends_cleanly_is_not_reset() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, mut app) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, true));
    server.stream.shutdown().await.unwrap();
    let mut got = Vec::new();
    app.read_to_end(&mut got).await.unwrap();
    app.write_all(b"pong").await.unwrap();
    drop(app);
    let mut reply = Vec::new();
    server.stream.read_to_end(&mut reply).await.unwrap();
    assert_eq!(reply, b"pong");
    pump.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_cancelled_plaintext_connection_resets_the_client_channel() {
    let (a, b) = tcp_pair().await;
    let (ours, mut theirs) = channels(a, b);
    let (mut visitor, _user) = tcp_pair().await;
    let open = tokio::spawn(async move {
        let mut early = Early::default();
        early.last_try();
        let opening = Opening {
            first: b"connect",
            prefix: &[],
            plaintext: true,
            reply_timeout: Duration::from_secs(5),
        };
        open_tcp(
            ours,
            &mut visitor,
            &opening,
            Meter::default(),
            &mut early,
            || {},
        )
        .await;
    });
    let first = theirs.reader.recv(&mut theirs.stream).await.unwrap();
    assert_eq!(first, Some(&b"connect"[..]));
    theirs
        .writer
        .send(&mut theirs.stream, &[Status::Ok as u8])
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    open.abort();
    assert!(sees_reset(&mut theirs.stream).await);
}

async fn opening_plaintext(
    ours: Channel,
    mut visitor: TcpStream,
) -> tokio::task::JoinHandle<Opened> {
    tokio::spawn(async move {
        let mut early = Early::default();
        early.last_try();
        let opening = Opening {
            first: b"connect",
            prefix: &[],
            plaintext: true,
            reply_timeout: Duration::from_secs(5),
        };
        open_tcp(
            ours,
            &mut visitor,
            &opening,
            Meter::default(),
            &mut early,
            || {},
        )
        .await
    })
}

#[tokio::test]
async fn a_user_gone_before_the_client_answered_resets_the_plaintext_channel() {
    let (a, b) = tcp_pair().await;
    let (ours, mut theirs) = channels(a, b);
    let (visitor, user) = tcp_pair().await;
    let open = opening_plaintext(ours, visitor).await;
    let first = theirs.reader.recv(&mut theirs.stream).await.unwrap();
    assert_eq!(first, Some(&b"connect"[..]));
    socket2::SockRef::from(&user)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(user);
    let opened = open.await.unwrap();
    assert!(matches!(opened, Opened::Gone(_)));
    assert!(sees_reset(&mut theirs.stream).await);
}

#[tokio::test]
async fn a_plaintext_channel_the_client_refused_is_closed_normally() {
    let (a, b) = tcp_pair().await;
    let (ours, mut theirs) = channels(a, b);
    let (visitor, _user) = tcp_pair().await;
    let open = opening_plaintext(ours, visitor).await;
    theirs.reader.recv(&mut theirs.stream).await.unwrap();
    theirs
        .writer
        .send(&mut theirs.stream, &[Status::Unreachable as u8])
        .await
        .unwrap();
    let opened = open.await.unwrap();
    assert!(matches!(
        opened,
        Opened::Refused(ForwardError::Status(Status::Unreachable))
    ));
    let mut buf = [0u8; 16];
    let end = tokio::time::timeout(Duration::from_secs(5), theirs.stream.read(&mut buf))
        .await
        .unwrap();
    assert!(matches!(end, Ok(0)), "{end:?}");
}

#[tokio::test]
async fn a_half_closed_channel_still_carries_the_target_reply() {
    let (a, b) = tcp_pair().await;
    let (ch, mut server) = channels(a, b);
    let (target, mut app) = tcp_pair().await;
    let pump = tokio::spawn(pump_tcp(ch, target, false));
    server
        .writer
        .send(&mut server.stream, b"ping")
        .await
        .unwrap();
    server.writer.send(&mut server.stream, &[]).await.unwrap();
    let mut got = Vec::new();
    app.read_to_end(&mut got).await.unwrap();
    assert_eq!(got, b"ping");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    app.write_all(b"pong").await.unwrap();
    drop(app);
    let reply = server
        .reader
        .recv(&mut server.stream)
        .await
        .unwrap()
        .unwrap()
        .to_vec();
    assert_eq!(reply, b"pong");
    assert_eq!(
        server.reader.recv(&mut server.stream).await.unwrap(),
        Some(&[][..])
    );
    drop(server);
    tokio::time::timeout(std::time::Duration::from_secs(5), pump)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_reply_to_a_peer_that_never_reads_gives_up() {
    let (a, _never_reads) = tcp_pair().await;
    let ch = channel(a);
    let Conn::Tcp(tcp) = &ch.stream else {
        unreachable!()
    };
    let junk = vec![0u8; 1 << 16];
    while tcp.try_write(&junk).is_ok() {}
    tokio::time::timeout(
        ANSWER_TIMEOUT + Duration::from_secs(3),
        send_and_close(ch, &[0u8; 60_000]),
    )
    .await
    .expect("the answer gives up");
}

async fn tcp_pair() -> (TcpStream, TcpStream) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (a, b) = tokio::join!(TcpStream::connect(l.local_addr().unwrap()), l.accept());
    (a.unwrap(), b.unwrap().0)
}

#[tokio::test]
async fn a_client_that_never_answers_nor_reads_is_given_up_in_time() {
    for plaintext in [false, true] {
        let (ours, _silent) = tcp_pair().await;
        let (mut visitor, mut user) = tcp_pair().await;
        let upload = tokio::spawn(async move {
            let _ = user.write_all(&vec![7u8; 64 << 20]).await;
            user
        });
        let mut early = Early::default();
        early.last_try();
        let opening = Opening {
            first: b"connect",
            prefix: &[],
            plaintext,
            reply_timeout: Duration::from_millis(300),
        };
        let started = Instant::now();
        let opened = tokio::time::timeout(
            Duration::from_secs(5),
            open_tcp(
                channel(ours),
                &mut visitor,
                &opening,
                Meter::default(),
                &mut early,
                || {},
            ),
        )
        .await
        .expect("the reply timeout holds while the upload waits");
        assert!(
            matches!(&opened, Opened::Refused(ForwardError::Io(e)) if e.kind() == io::ErrorKind::TimedOut),
            "plaintext {plaintext}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        upload.abort();
    }
}
