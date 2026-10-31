use std::io;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::early::Early;
use super::early::{Raw, Source, end_early, send_early};
use super::linger::Split;
use super::linger::{Graceful, close};
use super::{EARLY_READ, ForwardError, Meter, Opened, Opening, no_reply, pump_tcp, send_prefix};
use crate::wire::protocol::Status;
use crate::wire::record::{CHUNK, RecordReader, RecordWriter, open_pump, raw_pump, seal_pump};
use crate::wire::transport::Channel;
use crate::wire::transport::{Conn, ReadHalf, WriteHalf};

pub(super) async fn start(
    stream: &mut Conn,
    writer: &mut RecordWriter,
    opening: &Opening<'_>,
    early: &mut Early,
    plaintext: bool,
) -> io::Result<()> {
    writer.send(stream, opening.first).await?;
    send_prefix(stream, writer, opening.prefix, plaintext).await?;
    if !early.bytes.is_empty() {
        send_early(stream, writer, &early.bytes, plaintext).await?;
    }
    if !early.keep {
        early.bytes = Vec::new();
    }
    if early.ended {
        if plaintext {
            stream.close_normally();
        }
        end_early(stream, writer, plaintext).await?;
    }
    Ok(())
}

pub(super) enum Answer {
    Status(Status),
    Failed(ForwardError),
    Gone(io::Error),
}

pub(super) struct Client<'r, 'w, 'c> {
    pub(super) r: &'r mut ReadHalf<'c>,
    pub(super) reader: &'r mut RecordReader,
    pub(super) w: &'w mut WriteHalf<'c>,
    pub(super) writer: &'w mut RecordWriter,
}

fn reply_of(reply: io::Result<Option<&[u8]>>) -> Answer {
    match reply {
        Ok(Some(b)) => {
            Status::decode(b).map_or(Answer::Failed(ForwardError::Protocol), Answer::Status)
        }
        Ok(None) => Answer::Failed(ForwardError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the client closed its channel before replying",
        ))),
        Err(e) => Answer::Failed(ForwardError::Io(e)),
    }
}

pub(super) async fn answer(
    client: Client<'_, '_, '_>,
    source: &mut impl Source,
    early: &mut Early,
    plaintext: bool,
    timeout: Duration,
) -> Answer {
    let Client {
        r,
        reader,
        w,
        writer,
    } = client;
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut broken = false;
    loop {
        let room = if broken { 0 } else { early.room() };
        tokio::select! {
            biased;
            reply = reader.recv(r) => return reply_of(reply),
            _ = &mut deadline => return Answer::Failed(no_reply()),
            more = source.next(room), if room > 0 => {
                let more = match more {
                    Ok(more) => more,
                    Err(e) => return Answer::Gone(e),
                };
                let send = async {
                    match more {
                        Some(plain) => {
                            early.took(plain);
                            send_early(w, writer, plain, plaintext).await
                        }
                        None => {
                            early.ended = true;
                            end_early(w, writer, plaintext).await
                        }
                    }
                };
                tokio::pin!(send);
                let sent = tokio::select! {
                    biased;
                    sent = &mut send => sent,
                    reply = reader.recv(r) => match reply_of(reply) {
                        Answer::Status(Status::Ok) => match send.await {
                            Ok(()) => return Answer::Status(Status::Ok),
                            Err(e) => return Answer::Failed(ForwardError::Io(e)),
                        },
                        other => return other,
                    },
                    _ = &mut deadline => return Answer::Failed(no_reply()),
                };
                if sent.is_err() {
                    broken = true;
                }
            }
        }
    }
}

async fn answer_raw<R: tokio::io::AsyncRead + Unpin + Send>(
    client: Client<'_, '_, '_>,
    r: &mut R,
    early: &mut Early,
    plaintext: bool,
    timeout: Duration,
) -> Answer {
    let mut source = Raw {
        r,
        buf: [0; EARLY_READ],
    };
    answer(client, &mut source, early, plaintext, timeout).await
}

pub(super) fn ended_cleanly(done: &Result<Result<(), ForwardError>, Opened>) -> bool {
    matches!(
        done,
        Ok(Ok(())) | Err(Opened::Refused(ForwardError::Status(_)))
    )
}

pub(super) fn refused(answer: Answer) -> Option<Opened> {
    match answer {
        Answer::Status(Status::Ok) => None,
        Answer::Status(s) => Some(Opened::Refused(ForwardError::Status(s))),
        Answer::Failed(e) => Some(Opened::Refused(e)),
        Answer::Gone(e) => Some(Opened::Gone(Some(e))),
    }
}

pub async fn open_tcp<V: Split>(
    ch: Channel,
    visitor: &mut V,
    opening: &Opening<'_>,
    meter: Meter<'_>,
    early: &mut Early,
    accepted: impl FnOnce(),
) -> Opened {
    let Channel {
        mut stream,
        mut writer,
        mut reader,
    } = ch;
    let plaintext = opening.plaintext && stream.supports_half_close();
    if plaintext {
        stream.reset_on_close();
    }
    if let Err(e) = start(&mut stream, &mut writer, opening, early, plaintext).await {
        return Opened::Refused(ForwardError::Io(e));
    }
    let done = {
        let (mut cr, mut cw) = stream.split();
        let (vr, vw) = visitor.halves();
        let mut vr = meter.inbound(vr);
        let mut vw = meter.outbound(vw);
        let client = Client {
            r: &mut cr,
            reader: &mut reader,
            w: &mut cw,
            writer: &mut writer,
        };
        let answer = answer_raw(client, &mut vr, early, plaintext, opening.reply_timeout).await;
        match refused(answer) {
            Some(opened) => Err(opened),
            None => {
                accepted();
                early.accepted();
                let ended = early.ended;
                let up = async {
                    match (ended, plaintext) {
                        (true, _) => Ok(()),
                        (false, true) => raw_pump(&mut vr, &mut Graceful::new(&mut cw)).await,
                        (false, false) => seal_pump(&mut vr, &mut cw, &mut writer, CHUNK).await,
                    }
                };
                let down = async {
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
                Ok(tokio::try_join!(up, down).map(|_| ()))
            }
        }
    };
    if plaintext && ended_cleanly(&done) {
        stream.close_normally();
    }
    close(&mut stream).await;
    match done {
        Ok(result) => Opened::Done(result),
        Err(opened) => opened,
    }
}

pub async fn client_tcp<T: Split>(
    mut ch: Channel,
    target: T,
    plaintext: bool,
) -> Result<(), ForwardError> {
    ch.writer.send(&mut ch.stream, &[Status::Ok as u8]).await?;
    pump_tcp(ch, target, plaintext).await
}
