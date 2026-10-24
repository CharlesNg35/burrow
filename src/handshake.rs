use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::noise::{Initiator, MSG1_LEN, MSG2_LEN, Responder};
use crate::protocol::{ChannelKind, DecodeError, Hello, PREAMBLE_FIXED, Preamble};
use crate::record::{RecordReader, RecordWriter};
use crate::secret::Secret;
use crate::transport::{Channel, Conn};

const MAX_SMALL_RECORD: usize = 256;
pub const REFUSED: u8 = 0x00;
pub const MAX_REQUEST_RECORD: usize = 512;

#[derive(Debug)]
pub enum ClientHandshakeError {
    Io(io::Error),
    Rejected,
}

impl From<io::Error> for ClientHandshakeError {
    fn from(e: io::Error) -> Self {
        ClientHandshakeError::Io(e)
    }
}

pub async fn initiate(
    mut stream: Conn,
    preamble: &Preamble,
    secret: &Secret,
    hello: &Hello,
) -> Result<Channel, ClientHandshakeError> {
    let (init, msg1) = Initiator::start(preamble.cipher, secret, &preamble.prologue());
    let mut first = preamble.encode();
    first.extend_from_slice(&msg1);
    stream.write_all(&first).await?;
    stream.flush().await?;
    let mut msg2 = [0u8; MSG2_LEN];
    let mut filled = 0;
    while filled < MSG2_LEN {
        match stream.read(&mut msg2[filled..]).await {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
                ) =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        }
    }
    if msg2[..filled].starts_with(b"HTTP/") {
        return Err(ClientHandshakeError::Io(io::Error::other(
            "the server answered with HTTP; if it listens for WebSocket clients there, use ws:// or wss:// in the server address",
        )));
    }
    if filled == 1 && msg2[0] == REFUSED {
        return Err(ClientHandshakeError::Rejected);
    }
    if filled < MSG2_LEN {
        return Err(ClientHandshakeError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection closed before the server answered",
        )));
    }
    let session = init.finish(&msg2).map_err(|_| {
        ClientHandshakeError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "the answer is not from a burrow server that knows this name",
        ))
    })?;
    let (sealer, opener) = session.split();
    let mut writer = RecordWriter::new(sealer);
    writer.send(&mut stream, &hello.encode()).await?;
    Ok(Channel {
        stream,
        writer,
        reader: RecordReader::new(opener),
    })
}

#[derive(Debug)]
pub enum AcceptError {
    Io(io::Error),
    Decode(DecodeError),
    Rejected,
}

impl From<io::Error> for AcceptError {
    fn from(e: io::Error) -> Self {
        AcceptError::Io(e)
    }
}

impl From<DecodeError> for AcceptError {
    fn from(e: DecodeError) -> Self {
        AcceptError::Decode(e)
    }
}

pub struct Accepted {
    pub preamble: Preamble,
    pub hello: Hello,
    pub channel: Channel,
    pub secret: Secret,
}

pub async fn accept<F>(mut stream: Conn, secrets_for: F) -> Result<Accepted, AcceptError>
where
    F: FnOnce(ChannelKind, &str) -> Vec<Secret>,
{
    let mut head = [0u8; PREAMBLE_FIXED];
    stream.read_exact(&mut head).await?;
    let (kind, flags, name_len) = Preamble::decode_fixed(&head)?;
    let mut rest = [0u8; burrow_syntax::name::MAX_NAME_LEN + MSG1_LEN];
    let rest = &mut rest[..name_len + MSG1_LEN];
    stream.read_exact(rest).await?;
    let (name, msg1) = rest.split_at(name_len);
    let msg1: &[u8; MSG1_LEN] = msg1.try_into().expect("sliced to MSG1_LEN");
    let preamble = Preamble::from_parts(kind, flags, name)?;
    let candidates = secrets_for(preamble.kind, &preamble.name);
    let accepted = Responder::accept(preamble.cipher, &candidates, &preamble.prologue(), msg1)
        .and_then(|responder| {
            let secret = candidates[responder.secret_index()].clone();
            responder.reply().map(|reply| (secret, reply))
        });
    drop(candidates);
    let Ok((secret, (session, msg2))) = accepted else {
        let _ = stream.write_all(&[REFUSED]).await;
        let _ = stream.flush().await;
        return Err(AcceptError::Rejected);
    };
    stream.write_all(&msg2).await?;
    stream.flush().await?;
    let (sealer, opener) = session.split();
    let mut reader = RecordReader::new(opener);
    let hello = read_small(&mut stream, &mut reader, MAX_SMALL_RECORD).await?;
    let hello = Hello::decode(&hello)?;
    Ok(Accepted {
        preamble,
        hello,
        channel: Channel {
            stream,
            writer: RecordWriter::new(sealer),
            reader,
        },
        secret,
    })
}

pub async fn read_small(
    stream: &mut Conn,
    reader: &mut RecordReader,
    max: usize,
) -> Result<Vec<u8>, AcceptError> {
    let max = max.min(MAX_REQUEST_RECORD);
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;
    let len = u16::from_be_bytes(hdr) as usize;
    if !(crate::noise::TAG_LEN..=max).contains(&len) {
        return Err(AcceptError::Decode(DecodeError::TooLong));
    }
    let mut body = [0u8; MAX_REQUEST_RECORD];
    stream.read_exact(&mut body[..len]).await?;
    let mut plain = [0u8; MAX_REQUEST_RECORD];
    let n = reader
        .open_record(&body[..len], &mut plain)
        .ok_or(AcceptError::Rejected)?;
    Ok(plain[..n].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_an_explicit_refusal_counts_as_rejected() {
        use burrow_config::Cipher;
        async fn against(answer: &'static [u8]) -> Result<Channel, ClientHandshakeError> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut first = [0u8; 64];
                let _ = s.read(&mut first).await;
                let _ = s.write_all(answer).await;
            });
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let preamble = Preamble {
                kind: ChannelKind::Control,
                cipher: Cipher::Chachapoly,
                plaintext: false,
                name: "home".into(),
            };
            let hello = Hello::Control {
                version: crate::protocol::VERSION,
                instance: [0u8; 16],
            };
            initiate(stream.into(), &preamble, &Secret::generate(), &hello).await
        }
        assert!(matches!(
            against(&[REFUSED]).await,
            Err(ClientHandshakeError::Rejected)
        ));
        assert!(matches!(
            against(&[]).await,
            Err(ClientHandshakeError::Io(_))
        ));
        assert!(matches!(
            against(&[0x5a; MSG2_LEN]).await,
            Err(ClientHandshakeError::Io(_))
        ));
    }
}
