use std::future::Future;
use std::io;

use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{EARLY_READ, KEEP_EARLY};
use crate::wire::record::{CHUNK, RecordReader, RecordWriter};

pub struct Early {
    pub(super) bytes: Vec<u8>,
    pub(super) ended: bool,
    pub(super) keep: bool,
}

impl Default for Early {
    fn default() -> Self {
        Early {
            bytes: Vec::new(),
            ended: false,
            keep: true,
        }
    }
}

impl Early {
    pub fn last_try(&mut self) {
        self.keep = false;
    }

    pub fn sent_nothing(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(super) fn room(&self) -> usize {
        match (self.ended, self.keep) {
            (true, _) => 0,
            (false, true) => KEEP_EARLY.saturating_sub(self.bytes.len()),
            (false, false) => usize::MAX,
        }
    }

    pub(super) fn took(&mut self, plain: &[u8]) {
        if self.keep {
            self.bytes.extend_from_slice(plain);
        }
    }

    pub(super) fn accepted(&mut self) {
        self.bytes = Vec::new();
        self.keep = false;
    }
}

pub(super) trait Source {
    fn next(&mut self, room: usize) -> impl Future<Output = io::Result<Option<&[u8]>>> + Send;
}

pub(super) struct Raw<'a, R> {
    pub(super) r: &'a mut R,
    pub(super) buf: [u8; EARLY_READ],
}

impl<R: tokio::io::AsyncRead + Unpin + Send> Source for Raw<'_, R> {
    async fn next(&mut self, room: usize) -> io::Result<Option<&[u8]>> {
        let want = room.min(EARLY_READ);
        let n = self.r.read(&mut self.buf[..want]).await?;
        Ok((n > 0).then_some(&self.buf[..n]))
    }
}

pub(super) struct Sealed<'a, R> {
    pub(super) r: &'a mut R,
    pub(super) reader: &'a mut RecordReader,
    pub(super) counted: Option<&'a std::sync::atomic::AtomicU64>,
}

impl<R: tokio::io::AsyncRead + Unpin + Send> Source for Sealed<'_, R> {
    async fn next(&mut self, _room: usize) -> io::Result<Option<&[u8]>> {
        match self.reader.recv(self.r).await? {
            Some([]) => Ok(None),
            Some(plain) => {
                if let Some(c) = self.counted {
                    c.fetch_add(plain.len() as u64, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(Some(plain))
            }
            None => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "peer closed without an end-of-stream record",
            )),
        }
    }
}

pub(super) async fn send_early<W: AsyncWrite + Unpin>(
    w: &mut W,
    writer: &mut RecordWriter,
    plain: &[u8],
    plaintext: bool,
) -> io::Result<()> {
    if plaintext {
        w.write_all(plain).await?;
        return w.flush().await;
    }
    for chunk in plain.chunks(CHUNK) {
        writer.send(w, chunk).await?;
    }
    Ok(())
}

pub(super) async fn end_early<W: AsyncWrite + Unpin>(
    w: &mut W,
    writer: &mut RecordWriter,
    plaintext: bool,
) -> io::Result<()> {
    if plaintext {
        w.flush().await?;
        w.shutdown().await
    } else {
        writer.send(w, &[]).await
    }
}
