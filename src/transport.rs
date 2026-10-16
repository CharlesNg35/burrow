use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::noise::{MAX_PLAINTEXT, MAX_RECORD, Opener, Sealer, TAG_LEN};

pub const SMALL: usize = 512;
pub const CHUNK: usize = 16 * 1024;
const HEADER: usize = 2;
const READ_BUF: usize = 2 * (MAX_RECORD + HEADER);

async fn now<F: Future>(f: F) -> Option<F::Output> {
    tokio::select! {
        biased;
        v = f => Some(v),
        _ = std::future::ready(()) => None,
    }
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

pub struct RecordWriter {
    sealer: Sealer,
}

impl RecordWriter {
    pub fn new(sealer: Sealer) -> Self {
        RecordWriter { sealer }
    }

    pub fn seal_into(&mut self, plain: &[u8], out: &mut [u8]) -> usize {
        debug_assert!(plain.len() <= MAX_PLAINTEXT);
        let n = self.sealer.seal(plain, &mut out[HEADER..]);
        out[..HEADER].copy_from_slice(&(n as u16).to_be_bytes());
        n + HEADER
    }

    pub async fn send<W: AsyncWrite + Unpin>(&mut self, w: &mut W, plain: &[u8]) -> io::Result<()> {
        if plain.len() > MAX_PLAINTEXT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "record too large",
            ));
        }
        if plain.len() <= SMALL {
            let mut out = [0u8; SMALL + HEADER + TAG_LEN];
            let n = self.seal_into(plain, &mut out);
            w.write_all(&out[..n]).await?;
        } else {
            let mut out = vec![0u8; plain.len() + HEADER + TAG_LEN];
            let n = self.seal_into(plain, &mut out);
            w.write_all(&out[..n]).await?;
        }
        w.flush().await
    }
}

pub struct RecordReader {
    opener: Opener,
    buf: Option<Box<[u8]>>,
    out: Option<Box<[u8]>>,
    start: usize,
    end: usize,
    small: [u8; SMALL],
}

impl RecordReader {
    pub fn new(opener: Opener) -> Self {
        RecordReader {
            opener,
            buf: None,
            out: None,
            start: 0,
            end: 0,
            small: [0; SMALL],
        }
    }

    pub fn buffered(&self) -> &[u8] {
        match &self.buf {
            Some(b) => &b[self.start..self.end],
            None => &[],
        }
    }

    pub fn holds_buffers(&self) -> bool {
        self.buf.is_some() || self.out.is_some()
    }

    fn release(&mut self) {
        self.buf = None;
        self.out = None;
        self.start = 0;
        self.end = 0;
    }

    fn complete(&self) -> Option<usize> {
        let b = self.buf.as_ref()?;
        let avail = self.end - self.start;
        if avail < HEADER {
            return None;
        }
        let len = u16::from_be_bytes([b[self.start], b[self.start + 1]]) as usize;
        (avail >= HEADER + len).then_some(len)
    }

    pub async fn recv<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> io::Result<Option<&[u8]>> {
        loop {
            if let Some(len) = self.complete() {
                if len < TAG_LEN {
                    return Err(invalid("record shorter than its tag"));
                }
                let buf = self.buf.as_ref().expect("complete implies buffer");
                let rec = &buf[self.start + HEADER..self.start + HEADER + len];
                let plain_len = len - TAG_LEN;
                let out: &mut [u8] = if plain_len <= SMALL {
                    &mut self.small
                } else {
                    self.out
                        .get_or_insert_with(|| vec![0u8; MAX_PLAINTEXT].into_boxed_slice())
                };
                let n = self
                    .opener
                    .open(rec, out)
                    .ok_or_else(|| invalid("record failed authentication"))?;
                self.start += HEADER + len;
                let out: &[u8] = if plain_len <= SMALL {
                    &self.small
                } else {
                    self.out.as_deref().expect("allocated above")
                };
                return Ok(Some(&out[..n]));
            }
            self.fill(r).await?;
            if self.end == self.start && self.buf.is_none() {
                return Ok(None);
            }
        }
    }

    async fn fill<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> io::Result<()> {
        if self.start == self.end {
            if let Some(buf) = self.buf.as_mut() {
                self.start = 0;
                self.end = 0;
                match now(r.read(buf)).await {
                    Some(n) => {
                        let n = n?;
                        if n == 0 {
                            self.release();
                        } else {
                            self.end = n;
                        }
                        return Ok(());
                    }
                    None => self.release(),
                }
            }
            let mut hdr = [0u8; HEADER];
            let first = r.read(&mut hdr).await?;
            if first == 0 {
                return Ok(());
            }
            if first < HEADER {
                r.read_exact(&mut hdr[first..]).await?;
            }
            let mut buf = vec![0u8; READ_BUF].into_boxed_slice();
            buf[..HEADER].copy_from_slice(&hdr);
            self.buf = Some(buf);
            self.start = 0;
            self.end = HEADER;
            return Ok(());
        }
        let buf = self.buf.as_mut().expect("non-empty range implies buffer");
        if self.start > 0 && buf.len() - self.end < MAX_RECORD + HEADER {
            buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        let n = r.read(&mut buf[self.end..]).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed in the middle of a record",
            ));
        }
        self.end += n;
        Ok(())
    }
}

type SealBuffers = (Box<[u8]>, Box<[u8]>);

pub async fn seal_pump<R, W>(
    src: &mut R,
    dst: &mut W,
    writer: &mut RecordWriter,
    chunk: usize,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let chunk = chunk.clamp(SMALL, MAX_PLAINTEXT);
    let mut big: Option<SealBuffers> = None;
    loop {
        if let Some((plain, cipher)) = big.as_mut() {
            match now(src.read(plain)).await {
                Some(n) => {
                    let n = n?;
                    if n == 0 {
                        break;
                    }
                    let m = writer.seal_into(&plain[..n], cipher);
                    dst.write_all(&cipher[..m]).await?;
                }
                None => {
                    dst.flush().await?;
                    big = None;
                }
            }
            continue;
        }
        let mut small = [0u8; SMALL];
        let n = src.read(&mut small).await?;
        if n == 0 {
            break;
        }
        let mut out = [0u8; SMALL + HEADER + TAG_LEN];
        let m = writer.seal_into(&small[..n], &mut out);
        dst.write_all(&out[..m]).await?;
        if n == SMALL {
            big = Some((
                vec![0u8; chunk].into_boxed_slice(),
                vec![0u8; chunk + HEADER + TAG_LEN].into_boxed_slice(),
            ));
        } else {
            dst.flush().await?;
        }
    }
    let mut out = [0u8; HEADER + TAG_LEN];
    let m = writer.seal_into(&[], &mut out);
    dst.write_all(&out[..m]).await?;
    dst.flush().await
}

pub async fn open_pump<R, W>(src: &mut R, dst: &mut W, reader: &mut RecordReader) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    loop {
        match reader.recv(src).await? {
            Some([]) => {
                dst.flush().await?;
                return dst.shutdown().await;
            }
            Some(p) => dst.write_all(p).await?,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed without an end-of-stream record",
                ));
            }
        }
        if reader.buffered().is_empty() {
            dst.flush().await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Cipher;
    use crate::noise::{Initiator, Responder};
    use crate::secret::Secret;

    fn pair() -> ((RecordWriter, RecordReader), (RecordWriter, RecordReader)) {
        let s = Secret::generate();
        let (i, m1) = Initiator::start(Cipher::Chachapoly, &s, b"t");
        let r = Responder::accept(Cipher::Chachapoly, std::slice::from_ref(&s), b"t", &m1).unwrap();
        let (srv, m2) = r.reply().unwrap();
        let cli = i.finish(&m2).unwrap();
        let (cs, co) = cli.split();
        let (ss, so) = srv.split();
        (
            (RecordWriter::new(cs), RecordReader::new(co)),
            (RecordWriter::new(ss), RecordReader::new(so)),
        )
    }

    #[tokio::test]
    async fn records_of_all_sizes() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let sizes = [0usize, 1, 511, 512, 513, 16384, MAX_PLAINTEXT];
        let writer = tokio::spawn(async move {
            for s in sizes {
                let data: Vec<u8> = (0..s).map(|i| (i % 251) as u8).collect();
                w.send(&mut a, &data).await.unwrap();
            }
        });
        for s in sizes {
            let got = r.recv(&mut b).await.unwrap().unwrap().to_vec();
            assert_eq!(got.len(), s);
            assert!(got.iter().enumerate().all(|(i, v)| *v == (i % 251) as u8));
        }
        writer.await.unwrap();
        assert!(r.recv(&mut b).await.unwrap().is_none());
        assert!(!r.holds_buffers());
    }

    #[tokio::test]
    async fn pumps_carry_stream_and_eof() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut plain_in, mut plain_src) = tokio::io::duplex(1 << 16);
        let (mut wire_a, mut wire_b) = tokio::io::duplex(1 << 16);
        let (mut plain_dst, mut plain_out) = tokio::io::duplex(1 << 16);
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 256) as u8).collect();
        let expect = data.clone();
        let feeder = tokio::spawn(async move {
            plain_in.write_all(&data).await.unwrap();
            plain_in.shutdown().await.unwrap();
        });
        let seal = tokio::spawn(async move {
            seal_pump(&mut plain_src, &mut wire_a, &mut w, CHUNK)
                .await
                .unwrap();
        });
        let open = tokio::spawn(async move {
            open_pump(&mut wire_b, &mut plain_dst, &mut r)
                .await
                .unwrap();
        });
        let mut got = Vec::new();
        plain_out.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, expect);
        feeder.await.unwrap();
        seal.await.unwrap();
        open.await.unwrap();
    }

    #[tokio::test]
    async fn truncation_is_an_error() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        w.send(&mut a, b"hello").await.unwrap();
        a.write_all(&[0, 40, 1, 2]).await.unwrap();
        drop(a);
        assert_eq!(r.recv(&mut b).await.unwrap().unwrap(), b"hello");
        assert_eq!(
            r.recv(&mut b).await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn modified_record_is_rejected() {
        let ((mut w, _), (_, mut r)) = pair();
        let mut out = [0u8; 64];
        let n = w.seal_into(b"hello", &mut out);
        out[4] ^= 0x80;
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        a.write_all(&out[..n]).await.unwrap();
        assert_eq!(
            r.recv(&mut b).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn short_record_is_rejected() {
        let ((_, _), (_, mut r)) = pair();
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        a.write_all(&[0, 3, 1, 2, 3]).await.unwrap();
        assert!(r.recv(&mut b).await.is_err());
    }
}
