mod early;
mod linger;
mod open;

use std::io;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::metrics::{Counted, Traffic};
use crate::policy::ratelimit::{Limit, Throttled};
use crate::wire::protocol::Status;
use crate::wire::record::{CHUNK, SMALL, open_pump, raw_pump, relay_pump, seal_pump};
use crate::wire::transport::Channel;
use crate::wire::transport::Conn;
pub use early::Early;
use early::Sealed;
use linger::{Graceful, close};
pub use linger::{Linger, Split};
use open::{Client, answer, ended_cleanly, refused, start};
pub use open::{client_tcp, open_tcp};

pub(crate) const VISIT_REPLY: Duration = Duration::from_secs(45);

const KEEP_EARLY: usize = 64 * 1024;

const EARLY_READ: usize = SMALL;

const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Default)]
pub struct Meter<'a> {
    pub limit: Option<&'a Limit>,
    pub traffic: Option<&'a Traffic>,
}

impl<'a> Meter<'a> {
    fn throttle_in<S>(self, inner: S) -> Throttled<'a, S> {
        Throttled::new(inner, self.limit.map(|l| &l.inbound))
    }

    fn throttle_out<S>(self, inner: S) -> Throttled<'a, S> {
        Throttled::new(inner, self.limit.map(|l| &l.outbound))
    }

    fn inbound<S>(self, inner: S) -> Counted<'a, Throttled<'a, S>> {
        Counted::new(
            Throttled::new(inner, self.limit.map(|l| &l.inbound)),
            self.traffic.map(|t| &t.inbound),
        )
    }

    fn outbound<S>(self, inner: S) -> Counted<'a, Throttled<'a, S>> {
        Counted::new(
            Throttled::new(inner, self.limit.map(|l| &l.outbound)),
            self.traffic.map(|t| &t.outbound),
        )
    }
}

#[derive(Debug)]
pub enum ForwardError {
    Io(io::Error),
    Status(Status),
    Protocol,
}

impl From<io::Error> for ForwardError {
    fn from(e: io::Error) -> Self {
        ForwardError::Io(e)
    }
}

async fn send_prefix(
    stream: &mut Conn,
    writer: &mut crate::wire::record::RecordWriter,
    prefix: &[u8],
    plaintext: bool,
) -> io::Result<()> {
    match (prefix.is_empty(), plaintext) {
        (true, _) => Ok(()),
        (false, true) => stream.write_all(prefix).await,
        (false, false) => writer.send(stream, prefix).await,
    }
}

pub struct Opening<'a> {
    pub first: &'a [u8],
    pub prefix: &'a [u8],
    pub plaintext: bool,
    pub reply_timeout: Duration,
}

pub enum Opened {
    Done(Result<(), ForwardError>),
    Refused(ForwardError),
    Gone(Option<io::Error>),
}

pub(crate) fn no_reply() -> ForwardError {
    ForwardError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "the client did not answer in time",
    ))
}

pub async fn request_visit(ch: &mut Channel, request: &[u8]) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, request).await?;
    let reply = tokio::time::timeout(VISIT_REPLY, ch.reader.recv(&mut ch.stream))
        .await
        .map_err(|_| no_reply())?;
    let status = match reply? {
        Some(b) => Status::decode(b).map_err(|_| ForwardError::Protocol)?,
        None => return Err(ForwardError::Io(io::ErrorKind::UnexpectedEof.into())),
    };
    if status != Status::Ok {
        return Err(ForwardError::Status(status));
    }
    Ok(())
}

pub async fn pump_tcp<T: Split>(
    ch: Channel,
    mut target: T,
    plaintext: bool,
) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    let plaintext = plaintext && stream.supports_half_close();
    if plaintext {
        stream.reset_on_close();
    }
    target.reset_on_close(true);
    let done: io::Result<()> = {
        let (mut cr, mut cw) = stream.split();
        let (mut tr, mut tw) = target.halves();
        let up = async {
            if plaintext {
                raw_pump(&mut tr, &mut Graceful::new(&mut cw)).await
            } else {
                seal_pump(&mut tr, &mut cw, &mut writer, CHUNK).await
            }
        };
        use std::sync::atomic::Ordering::Relaxed;
        let up_done = std::sync::atomic::AtomicBool::new(false);
        let down_done = std::sync::atomic::AtomicBool::new(false);
        let down = async {
            let mut tw = Graceful::new(&mut tw);
            if plaintext {
                let rest = reader.take_buffered();
                tw.write_all(&rest).await?;
                raw_pump(&mut cr, &mut tw).await?;
            } else {
                open_pump(&mut cr, &mut tw, &mut reader).await?;
            }
            down_done.store(true, Relaxed);
            if up_done.load(Relaxed) {
                if !plaintext {
                    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
                        while let Ok(Some(_)) = reader.recv(&mut cr).await {}
                    })
                    .await;
                }
                return Ok(());
            }
            if plaintext {
                std::future::pending::<()>().await;
            }
            while reader.recv(&mut cr).await?.is_some() {}
            Ok(())
        };
        tokio::pin!(up, down);
        loop {
            tokio::select! {
                r = &mut up, if !up_done.load(Relaxed) => {
                    if let Err(e) = r {
                        break Err(e);
                    }
                    up_done.store(true, Relaxed);
                    if down_done.load(Relaxed) {
                        if plaintext {
                            break Ok(());
                        }
                        break tokio::time::timeout(CLOSE_TIMEOUT, &mut down)
                            .await
                            .unwrap_or(Ok(()));
                    }
                }
                r = &mut down => break r,
            }
        }
    };
    if done.is_ok() {
        target.reset_on_close(false);
        if plaintext {
            stream.close_normally();
        }
    }
    close(&mut stream).await;
    done?;
    Ok(())
}

pub async fn relay(
    visitor: &mut Channel,
    client: Channel,
    opening: &Opening<'_>,
    meter: Meter<'_>,
    early: &mut Early,
    accepted: impl FnOnce(),
) -> Opened {
    let Channel {
        stream: vs,
        writer: vwriter,
        reader: vreader,
    } = visitor;
    let Channel {
        stream: mut cs,
        writer: mut cwriter,
        reader: mut creader,
    } = client;
    let plaintext = opening.plaintext && cs.supports_half_close();
    if plaintext {
        cs.reset_on_close();
    }
    if let Err(e) = start(&mut cs, &mut cwriter, opening, early, plaintext).await {
        return Opened::Refused(ForwardError::Io(e));
    }
    let done = {
        let (mut cr, mut cw) = cs.split();
        let (vr, vw) = vs.split();
        let mut vr = meter.throttle_in(vr);
        let mut vw = meter.throttle_out(vw);
        let inbound = meter.traffic.map(|t| &t.inbound);
        let client = Client {
            r: &mut cr,
            reader: &mut creader,
            w: &mut cw,
            writer: &mut cwriter,
        };
        let source = &mut Sealed {
            r: &mut vr,
            reader: &mut *vreader,
            counted: inbound,
        };
        let answer = answer(client, source, early, plaintext, opening.reply_timeout).await;
        match refused(answer) {
            Some(opened) => Err(opened),
            None => {
                accepted();
                early.accepted();
                let ended = early.ended;
                let raw = |bytes| if plaintext { bytes } else { None };
                let mut cw = Counted::new(Graceful::new(cw), raw(inbound));
                let sealed = |bytes| if plaintext { None } else { bytes };
                let up = async {
                    match (ended, plaintext) {
                        (true, _) => Ok(()),
                        (false, true) => open_pump(&mut vr, &mut cw, vreader).await,
                        (false, false) => {
                            relay_pump(&mut vr, vreader, &mut cw, &mut cwriter, sealed(inbound))
                                .await
                        }
                    }
                };
                let down = async {
                    vwriter.send(&mut vw, &[Status::Ok as u8]).await?;
                    if plaintext {
                        let outbound = meter.traffic.map(|t| &t.outbound);
                        let rest = creader.take_buffered();
                        if let Some(c) = outbound {
                            c.fetch_add(rest.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        }
                        for chunk in rest.chunks(CHUNK) {
                            vwriter.send(&mut vw, chunk).await?;
                        }
                        let mut cr = Counted::new(&mut cr, outbound);
                        seal_pump(&mut cr, &mut vw, vwriter, CHUNK).await?;
                    } else {
                        let counted = sealed(meter.traffic.map(|t| &t.outbound));
                        relay_pump(&mut cr, &mut creader, &mut vw, vwriter, counted).await?;
                    }
                    Ok(())
                };
                let up = async { up.await.map_err(ForwardError::Io) };
                Ok(tokio::try_join!(up, down).map(|_| ()))
            }
        }
    };
    if plaintext && ended_cleanly(&done) {
        cs.close_normally();
    }
    match done {
        Ok(result) => {
            tokio::join!(close(vs), close(&mut cs));
            Opened::Done(result)
        }
        Err(opened) => {
            close(&mut cs).await;
            opened
        }
    }
}

pub async fn reply(ch: Channel, status: Status) {
    send_and_close(ch, &[status as u8]).await;
}

pub async fn refuse(mut ch: Channel, status: Status) {
    let byte = [status as u8];
    let sent = ch.writer.send(&mut ch.stream, &byte);
    if !matches!(tokio::time::timeout(ANSWER_TIMEOUT, sent).await, Ok(Ok(()))) {
        return;
    }
    let mut drain = [0u8; 4096];
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
        while matches!(ch.stream.read(&mut drain).await, Ok(n) if n > 0) {}
    })
    .await;
}

pub async fn send_and_close(mut ch: Channel, bytes: &[u8]) {
    let sent = async {
        ch.writer.send(&mut ch.stream, bytes).await?;
        ch.stream.shutdown().await
    };
    let _ = tokio::time::timeout(ANSWER_TIMEOUT, sent).await;
}
#[cfg(test)]
mod tests {
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
}
