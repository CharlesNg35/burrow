use std::io;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::handshake::Channel;
use crate::protocol::{Connect, Status};
use crate::transport::{CHUNK, open_pump, raw_pump, seal_pump};

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

pub async fn server_tcp(
    ch: Channel,
    mut visitor: TcpStream,
    connect: &Connect,
    plaintext: bool,
) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    writer.send(&mut stream, &connect.encode()).await?;
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
    tokio::try_join!(up, down).map(|_| ())
}

pub async fn client_tcp(
    ch: Channel,
    mut target: TcpStream,
    plaintext: bool,
) -> Result<(), ForwardError> {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    writer.send(&mut stream, &[Status::Ok as u8]).await?;
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
    tokio::try_join!(up, down)?;
    Ok(())
}

pub async fn refuse(mut ch: Channel, status: Status) {
    let _ = ch.writer.send(&mut ch.stream, &[status as u8]).await;
    let _ = ch.stream.shutdown().await;
}
