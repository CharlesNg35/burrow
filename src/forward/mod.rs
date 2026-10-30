mod early;
mod incoming;
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
pub use incoming::Incoming;
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

pub async fn send_and_close(ch: Channel, bytes: &[u8]) {
    send_then(ch, bytes, false).await;
}

pub async fn send_and_finish(ch: Channel, bytes: &[u8]) {
    send_then(ch, bytes, true).await;
}

async fn send_then(mut ch: Channel, bytes: &[u8], finish: bool) {
    let sent = async {
        ch.writer.send(&mut ch.stream, bytes).await?;
        if finish {
            ch.stream.flush().await?;
            ch.stream.finish().await
        } else {
            ch.stream.shutdown().await
        }
    };
    let _ = tokio::time::timeout(ANSWER_TIMEOUT, sent).await;
}
#[cfg(test)]
mod tests;
