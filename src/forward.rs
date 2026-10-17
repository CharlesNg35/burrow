use std::io;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::handshake::Channel;
use crate::metrics::{Counted, Traffic};
use crate::protocol::{Connect, Status};
use crate::ratelimit::{Limit, Throttled};
use crate::record::{CHUNK, open_pump, raw_pump, relay_pump, seal_pump};
use crate::transport::Conn;

const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

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

pub trait Split {
    type Read<'a>: tokio::io::AsyncRead + Unpin + Send
    where
        Self: 'a;
    type Write<'a>: tokio::io::AsyncWrite + Unpin + Send
    where
        Self: 'a;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>);
}

impl Split for TcpStream {
    type Read<'a> = tokio::net::tcp::ReadHalf<'a>;
    type Write<'a> = tokio::net::tcp::WriteHalf<'a>;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        self.split()
    }
}

#[cfg(unix)]
impl Split for tokio::net::UnixStream {
    type Read<'a> = tokio::net::unix::ReadHalf<'a>;
    type Write<'a> = tokio::net::unix::WriteHalf<'a>;
    fn halves(&mut self) -> (Self::Read<'_>, Self::Write<'_>) {
        self.split()
    }
}

async fn close(stream: &mut Conn) {
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, stream.finish()).await;
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
    writer: &mut crate::record::RecordWriter,
    prefix: &[u8],
    plaintext: bool,
) -> io::Result<()> {
    match (prefix.is_empty(), plaintext) {
        (true, _) => Ok(()),
        (false, true) => stream.write_all(prefix).await,
        (false, false) => writer.send(stream, prefix).await,
    }
}

pub async fn open_tcp(
    ch: Channel,
    mut visitor: TcpStream,
    first: &[u8],
    prefix: &[u8],
    plaintext: bool,
    meter: Meter<'_>,
) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    writer.send(&mut stream, first).await?;
    let plaintext = plaintext && stream.supports_half_close();
    send_prefix(&mut stream, &mut writer, prefix, plaintext).await?;
    let (mut cr, mut cw) = stream.split();
    let (vr, vw) = visitor.split();
    let mut vr = meter.inbound(vr);
    let mut vw = meter.outbound(vw);
    let up = async {
        if plaintext {
            raw_pump(&mut vr, &mut cw).await
        } else {
            seal_pump(&mut vr, &mut cw, &mut writer, CHUNK).await
        }
    };
    let down = async {
        let status = match reader.recv(&mut cr).await? {
            Some(b) => Status::decode(b).map_err(|_| ForwardError::Protocol)?,
            None => return Err(ForwardError::Protocol),
        };
        if status != Status::Ok {
            return Err(ForwardError::Status(status));
        }
        if plaintext {
            let rest = reader.take_buffered();
            vw.write_all(&rest).await?;
            raw_pump(&mut cr, &mut vw).await?;
        } else {
            open_pump(&mut cr, &mut vw, &mut reader).await?;
        }
        Ok(())
    };
    let up = async { up.await.map_err(ForwardError::Io) };
    let done = tokio::try_join!(up, down).map(|_| ());
    close(&mut stream).await;
    done
}

pub async fn client_tcp<T: Split>(
    mut ch: Channel,
    target: T,
    plaintext: bool,
) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, &[Status::Ok as u8]).await?;
    pump_tcp(ch, target, plaintext).await
}

pub async fn request_visit(ch: &mut Channel, request: &[u8]) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, request).await?;
    let status = match ch.reader.recv(&mut ch.stream).await? {
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
    let (mut cr, mut cw) = stream.split();
    let (mut tr, mut tw) = target.halves();
    let up = async {
        if plaintext {
            raw_pump(&mut tr, &mut cw).await
        } else {
            seal_pump(&mut tr, &mut cw, &mut writer, CHUNK).await
        }
    };
    let down = async {
        if plaintext {
            let rest = reader.take_buffered();
            tw.write_all(&rest).await?;
            raw_pump(&mut cr, &mut tw).await
        } else {
            open_pump(&mut cr, &mut tw, &mut reader).await
        }
    };
    let done = tokio::try_join!(up, down);
    close(&mut stream).await;
    done?;
    Ok(())
}

pub async fn relay(
    visitor: Channel,
    client: Channel,
    connect: &Connect,
    prefix: &[u8],
    plaintext: bool,
    meter: Meter<'_>,
) -> Result<(), ForwardError> {
    let Channel {
        stream: mut vs,
        writer: mut vwriter,
        reader: mut vreader,
    } = visitor;
    let Channel {
        stream: mut cs,
        writer: mut cwriter,
        reader: mut creader,
    } = client;
    cwriter.send(&mut cs, &connect.encode()).await?;
    let plaintext = plaintext && cs.supports_half_close();
    send_prefix(&mut cs, &mut cwriter, prefix, plaintext).await?;
    let (mut cr, cw) = cs.split();
    let raw = |bytes| if plaintext { bytes } else { None };
    let mut cw = Counted::new(cw, raw(meter.traffic.map(|t| &t.inbound)));
    let sealed = |bytes| if plaintext { None } else { bytes };
    let (vr, vw) = vs.split();
    let mut vr = meter.throttle_in(vr);
    let mut vw = meter.throttle_out(vw);
    let up = async {
        if plaintext {
            open_pump(&mut vr, &mut cw, &mut vreader).await
        } else {
            let counted = sealed(meter.traffic.map(|t| &t.inbound));
            relay_pump(&mut vr, &mut vreader, &mut cw, &mut cwriter, counted).await
        }
    };
    let down = async {
        let status = match creader.recv(&mut cr).await {
            Ok(Some(b)) => Status::decode(b).map_err(|_| ForwardError::Protocol)?,
            Ok(None) | Err(_) => {
                let _ = vwriter.send(&mut vw, &[Status::Offline as u8]).await;
                return Err(ForwardError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the client closed its channel before replying",
                )));
            }
        };
        vwriter.send(&mut vw, &[status as u8]).await?;
        if status != Status::Ok {
            return Err(ForwardError::Status(status));
        }
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
            seal_pump(&mut cr, &mut vw, &mut vwriter, CHUNK).await?;
        } else {
            let counted = sealed(meter.traffic.map(|t| &t.outbound));
            relay_pump(&mut cr, &mut creader, &mut vw, &mut vwriter, counted).await?;
        }
        Ok(())
    };
    let up = async { up.await.map_err(ForwardError::Io) };
    let done = tokio::try_join!(up, down).map(|_| ());
    tokio::join!(close(&mut vs), close(&mut cs));
    done
}

pub async fn reply(mut ch: Channel, status: Status) {
    let _ = ch.writer.send(&mut ch.stream, &[status as u8]).await;
    let _ = ch.stream.shutdown().await;
}
