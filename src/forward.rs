use std::io;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::handshake::Channel;
use crate::protocol::{Connect, Status};
use crate::record::{CHUNK, open_pump, raw_pump, relay_pump, seal_pump};
use crate::transport::Conn;

const CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

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

pub async fn open_tcp(
    ch: Channel,
    mut visitor: TcpStream,
    first: &[u8],
    plaintext: bool,
) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    writer.send(&mut stream, first).await?;
    let plaintext = plaintext && stream.supports_half_close();
    let (mut cr, mut cw) = stream.split();
    let (mut vr, mut vw) = visitor.split();
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

pub async fn client_tcp(
    mut ch: Channel,
    target: TcpStream,
    plaintext: bool,
) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, &[Status::Ok as u8]).await?;
    pump_tcp(ch, target, plaintext).await
}

pub async fn visit_tcp(
    mut ch: Channel,
    local: TcpStream,
    request: &[u8],
) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, request).await?;
    let status = match ch.reader.recv(&mut ch.stream).await? {
        Some(b) => Status::decode(b).map_err(|_| ForwardError::Protocol)?,
        None => return Err(ForwardError::Protocol),
    };
    if status != Status::Ok {
        return Err(ForwardError::Status(status));
    }
    pump_tcp(ch, local, false).await
}

async fn pump_tcp(ch: Channel, mut target: TcpStream, plaintext: bool) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    let plaintext = plaintext && stream.supports_half_close();
    let (mut cr, mut cw) = stream.split();
    let (mut tr, mut tw) = target.split();
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
    plaintext: bool,
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
    let (mut cr, mut cw) = cs.split();
    let (mut vr, mut vw) = vs.split();
    let up = async {
        if plaintext {
            open_pump(&mut vr, &mut cw, &mut vreader).await
        } else {
            relay_pump(&mut vr, &mut vreader, &mut cw, &mut cwriter).await
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
            let rest = creader.take_buffered();
            for chunk in rest.chunks(CHUNK) {
                vwriter.send(&mut vw, chunk).await?;
            }
            seal_pump(&mut cr, &mut vw, &mut vwriter, CHUNK).await?;
        } else {
            relay_pump(&mut cr, &mut creader, &mut vw, &mut vwriter).await?;
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
