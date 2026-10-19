use std::time::Duration;

use burrow_admin_proto::frame::{self, AdminOpen, Assembler, Frame};
use tokio::io::AsyncWriteExt;

use super::{DialError, Dialer};
use crate::protocol::{ChannelKind, Hello, VERSION};
use crate::record::{RecordReader, RecordWriter};
use crate::transport::{OwnedReadHalf, OwnedWriteHalf};

const ANSWER: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminRefusal {
    Forbidden,
    Busy,
    NoAdmin,
    Explained(String),
    Other(u8),
}

#[derive(Debug)]
pub enum AdminOpenError {
    Dial(DialError),
    Refused(AdminRefusal),
    Lost(String),
}

pub struct AdminRx {
    reader: RecordReader,
    rd: OwnedReadHalf,
    assembler: Assembler,
}

pub struct AdminTx {
    writer: RecordWriter,
    wr: OwnedWriteHalf,
}

impl AdminRx {
    pub async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            let Some(rec) = self.reader.recv(&mut self.rd).await? else {
                return Ok(None);
            };
            if rec.is_empty() {
                return Ok(None);
            }
            match self.assembler.push(rec) {
                Ok(Frame::Message(m)) => return Ok(Some(m)),
                Ok(_) => continue,
                Err(e) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        e.to_string(),
                    ));
                }
            }
        }
    }
}

impl AdminTx {
    pub async fn send(&mut self, message: &[u8]) -> std::io::Result<()> {
        let records = frame::chunks(message)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?;
        for r in records {
            self.writer.send(&mut self.wr, &r).await?;
        }
        Ok(())
    }

    pub async fn send_record(&mut self, record: &[u8]) -> std::io::Result<()> {
        self.writer.send(&mut self.wr, record).await
    }

    pub async fn ping(&mut self) -> std::io::Result<()> {
        self.writer.send(&mut self.wr, &frame::ping()).await
    }

    pub async fn close(mut self) {
        let _ = self.writer.send(&mut self.wr, &[]).await;
        let _ = self.wr.shutdown().await;
    }
}

impl Dialer {
    pub async fn open_admin(
        &self,
        open: &AdminOpen,
    ) -> Result<(AdminRx, AdminTx, u8), AdminOpenError> {
        let mut ch = self
            .open(ChannelKind::Visit, Hello::Visit { version: VERSION })
            .await
            .map_err(AdminOpenError::Dial)?;
        let first = open
            .encode()
            .map_err(|e| AdminOpenError::Lost(e.to_string()))?;
        ch.writer
            .send(&mut ch.stream, &first)
            .await
            .map_err(|e| AdminOpenError::Lost(e.to_string()))?;
        let answer = tokio::time::timeout(ANSWER, ch.reader.recv(&mut ch.stream))
            .await
            .map_err(|_| AdminOpenError::Lost(format!("{} did not answer in time", self.server)))?
            .map_err(|e| AdminOpenError::Lost(e.to_string()))?
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                AdminOpenError::Lost(format!("{} closed the connection", self.server))
            })?;
        if let Some(v) = frame::decode_accepted(&answer) {
            let (rd, wr) = ch.stream.into_split();
            return Ok((
                AdminRx {
                    reader: ch.reader,
                    rd,
                    assembler: Assembler::new(),
                },
                AdminTx {
                    writer: ch.writer,
                    wr,
                },
                v,
            ));
        }
        let refusal = match answer.as_slice() {
            [5] => AdminRefusal::Forbidden,
            [7] => AdminRefusal::Busy,
            [4] => {
                let text =
                    tokio::time::timeout(Duration::from_secs(2), ch.reader.recv(&mut ch.stream))
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .flatten()
                        .filter(|t| !t.is_empty())
                        .map(|t| String::from_utf8_lossy(t).into_owned());
                match text {
                    Some(t) => AdminRefusal::Explained(t),
                    None => AdminRefusal::NoAdmin,
                }
            }
            [x] => AdminRefusal::Other(*x),
            _ => AdminRefusal::Other(0xff),
        };
        Err(AdminOpenError::Refused(refusal))
    }
}
