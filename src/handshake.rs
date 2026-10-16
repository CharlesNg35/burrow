use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::noise::{Initiator, MSG1_LEN, MSG2_LEN, Responder};
use crate::protocol::{DecodeError, Hello, PREAMBLE_FIXED, Preamble};
use crate::record::{RecordReader, RecordWriter};
use crate::secret::Secret;
use crate::transport::Conn;

const MAX_SMALL_RECORD: usize = 256;

pub struct Channel {
    pub stream: Conn,
    pub writer: RecordWriter,
    pub reader: RecordReader,
}

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
    match stream.read_exact(&mut msg2).await {
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
            ) =>
        {
            return Err(ClientHandshakeError::Rejected);
        }
        Err(e) => return Err(e.into()),
    }
    let session = init
        .finish(&msg2)
        .map_err(|_| ClientHandshakeError::Rejected)?;
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
    F: FnOnce(&str) -> Vec<Secret>,
{
    let mut head = [0u8; PREAMBLE_FIXED];
    stream.read_exact(&mut head).await?;
    let (kind, flags, name_len) = Preamble::decode_fixed(&head)?;
    let mut rest = [0u8; crate::config::check::MAX_NAME_LEN + MSG1_LEN];
    let rest = &mut rest[..name_len + MSG1_LEN];
    stream.read_exact(rest).await?;
    let (name, msg1) = rest.split_at(name_len);
    let msg1: &[u8; MSG1_LEN] = msg1.try_into().expect("sliced to MSG1_LEN");
    let preamble = Preamble::from_parts(kind, flags, name)?;
    let candidates = secrets_for(&preamble.name);
    let responder = Responder::accept(preamble.cipher, &candidates, &preamble.prologue(), msg1)
        .map_err(|_| AcceptError::Rejected)?;
    let secret = candidates[responder.secret_index()].clone();
    drop(candidates);
    let (session, msg2) = responder.reply().map_err(|_| AcceptError::Rejected)?;
    stream.write_all(&msg2).await?;
    stream.flush().await?;
    let (sealer, opener) = session.split();
    let mut reader = RecordReader::new(opener);
    let hello = read_small(&mut stream, &mut reader).await?;
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

async fn read_small(stream: &mut Conn, reader: &mut RecordReader) -> Result<Vec<u8>, AcceptError> {
    let mut hdr = [0u8; 2];
    stream.read_exact(&mut hdr).await?;
    let len = u16::from_be_bytes(hdr) as usize;
    if !(crate::noise::TAG_LEN..=MAX_SMALL_RECORD).contains(&len) {
        return Err(AcceptError::Decode(DecodeError::TooLong));
    }
    let mut body = [0u8; MAX_SMALL_RECORD];
    stream.read_exact(&mut body[..len]).await?;
    let mut plain = [0u8; MAX_SMALL_RECORD];
    let n = reader
        .open_record(&body[..len], &mut plain)
        .ok_or(AcceptError::Rejected)?;
    Ok(plain[..n].to_vec())
}
