use std::future::Future;
use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::wire::noise::{MAX_PLAINTEXT, MAX_RECORD, Opener, Sealer, TAG_LEN};
use pool::Big;

mod pool;

pub const SMALL: usize = 512;
pub const CHUNK: usize = MAX_PLAINTEXT;
const RAW_CHUNK: usize = 64 * 1024;
const HEADER: usize = 2;
const FIRST_READ: usize = SMALL + HEADER + TAG_LEN;

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
        if plain.len() > SMALL {
            return self.send_with(w, plain, &mut Vec::new()).await;
        }
        let mut out = [0u8; SMALL + HEADER + TAG_LEN];
        let n = self.seal_into(plain, &mut out);
        w.write_all(&out[..n]).await?;
        w.flush().await
    }

    pub async fn send_with<W: AsyncWrite + Unpin>(
        &mut self,
        w: &mut W,
        plain: &[u8],
        scratch: &mut Vec<u8>,
    ) -> io::Result<()> {
        if plain.len() > MAX_PLAINTEXT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "record too large",
            ));
        }
        scratch.resize(plain.len() + HEADER + TAG_LEN, 0);
        let n = self.seal_into(plain, scratch);
        w.write_all(&scratch[..n]).await?;
        w.flush().await
    }
}

pub struct RecordReader {
    opener: Opener,
    buf: Option<Big>,
    out: Option<Big>,
    start: usize,
    end: usize,
    small: [u8; SMALL],
    head: [u8; FIRST_READ],
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
            head: [0; FIRST_READ],
        }
    }

    pub fn open_record(&mut self, record: &[u8], out: &mut [u8]) -> Option<usize> {
        self.opener.open(record, out)
    }

    pub fn buffered(&self) -> &[u8] {
        match &self.buf {
            Some(b) => &b[self.start..self.end],
            None => &[],
        }
    }

    pub fn take_buffered(&mut self) -> Vec<u8> {
        let rest = self.buffered().to_vec();
        self.release();
        rest
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
                    &mut self.out.get_or_insert_with(Big::take)[..]
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
                match now(r.read(&mut buf[..])).await {
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
            let n = r.read(&mut self.head).await?;
            if n == 0 {
                return Ok(());
            }
            let mut buf = Big::take();
            buf[..n].copy_from_slice(&self.head[..n]);
            self.buf = Some(buf);
            self.start = 0;
            self.end = n;
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

type SealBuffers = (Big, Big);

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
            match now(src.read(&mut plain[..chunk])).await {
                Some(n) => {
                    let n = n?;
                    if n == 0 {
                        break;
                    }
                    let m = writer.seal_into(&plain[..n], &mut cipher[..]);
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
        if n == SMALL {
            let (bufs, ended) = seal_first_big(src, dst, writer, &small, chunk).await?;
            if ended {
                break;
            }
            big = Some(bufs);
            continue;
        }
        let mut out = [0u8; SMALL + HEADER + TAG_LEN];
        let m = writer.seal_into(&small[..n], &mut out);
        dst.write_all(&out[..m]).await?;
        dst.flush().await?;
    }
    let mut out = [0u8; HEADER + TAG_LEN];
    let m = writer.seal_into(&[], &mut out);
    dst.write_all(&out[..m]).await?;
    dst.flush().await
}

async fn seal_first_big<R, W>(
    src: &mut R,
    dst: &mut W,
    writer: &mut RecordWriter,
    first: &[u8],
    chunk: usize,
) -> io::Result<(SealBuffers, bool)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut plain, mut cipher) = (Big::take(), Big::take());
    let n = first.len();
    plain[..n].copy_from_slice(first);
    let (more, ended) = match now(src.read(&mut plain[n..chunk])).await {
        Some(r) => {
            let more = r?;
            (more, more == 0)
        }
        None => (0, false),
    };
    let m = writer.seal_into(&plain[..n + more], &mut cipher[..]);
    dst.write_all(&cipher[..m]).await?;
    Ok(((plain, cipher), ended))
}

pub async fn raw_pump<R, W>(src: &mut R, dst: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut big: Option<Big> = None;
    loop {
        if let Some(buf) = big.as_mut() {
            match now(src.read(&mut buf[..RAW_CHUNK])).await {
                Some(n) => {
                    let n = n?;
                    if n == 0 {
                        break;
                    }
                    dst.write_all(&buf[..n]).await?;
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
        dst.write_all(&small[..n]).await?;
        if n == SMALL {
            big = Some(Big::take());
        } else {
            dst.flush().await?;
        }
    }
    dst.flush().await?;
    dst.shutdown().await
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

pub async fn relay_pump<R, W>(
    src: &mut R,
    reader: &mut RecordReader,
    dst: &mut W,
    writer: &mut RecordWriter,
    counted: Option<&std::sync::atomic::AtomicU64>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut small = [0u8; SMALL + HEADER + TAG_LEN];
    let mut big: Option<Big> = None;
    loop {
        let plain = match reader.recv(src).await? {
            Some(p) => p,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed without an end-of-stream record",
                ));
            }
        };
        let end = plain.is_empty();
        if let Some(c) = counted {
            c.fetch_add(plain.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let out: &mut [u8] = if plain.len() <= SMALL {
            &mut small
        } else {
            &mut big.get_or_insert_with(Big::take)[..]
        };
        let n = writer.seal_into(plain, out);
        dst.write_all(&out[..n]).await?;
        if end {
            return dst.flush().await;
        }
        if reader.buffered().is_empty() {
            dst.flush().await?;
            if !reader.holds_buffers() {
                big = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::noise::{Initiator, Responder};
    use crate::wire::secret::Secret;
    use burrow_config::Cipher;

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
    async fn a_read_stopped_in_the_middle_of_a_header_loses_nothing() {
        let ((mut w, _), (_, mut r)) = pair();
        let mut sealed = vec![0u8; SMALL + HEADER + TAG_LEN];
        let n = w.seal_into(b"hello", &mut sealed);
        let mut more = vec![0u8; SMALL + HEADER + TAG_LEN];
        let m = w.seal_into(b"again", &mut more);
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        for (bytes, want) in [(&sealed[..n], &b"hello"[..]), (&more[..m], &b"again"[..])] {
            a.write_all(&bytes[..1]).await.unwrap();
            let stopped =
                tokio::time::timeout(std::time::Duration::from_millis(50), r.recv(&mut b)).await;
            assert!(stopped.is_err());
            a.write_all(&bytes[1..]).await.unwrap();
            let got = tokio::time::timeout(std::time::Duration::from_secs(5), r.recv(&mut b))
                .await
                .expect("the rest of the record is read");
            assert_eq!(got.unwrap(), Some(want));
        }
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
    async fn a_one_record_buffer_reads_records_split_anywhere_across_reads() {
        let ((mut w, _), (_, mut r)) = pair();
        let sizes = [
            600usize,
            MAX_PLAINTEXT,
            3,
            MAX_PLAINTEXT,
            40_000,
            MAX_PLAINTEXT,
            0,
        ];
        let mut wire = Vec::new();
        for s in sizes {
            let data: Vec<u8> = (0..s).map(|i| (i % 251) as u8).collect();
            w.send(&mut wire, &data).await.unwrap();
        }
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let writer = tokio::spawn(async move {
            for piece in wire.chunks(40_001) {
                a.write_all(piece).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        for s in sizes {
            let got = r.recv(&mut b).await.unwrap().unwrap().to_vec();
            assert_eq!(got.len(), s);
            assert!(got.iter().enumerate().all(|(i, v)| *v == (i % 251) as u8));
        }
        writer.await.unwrap();
    }

    struct Reads<R>(R, usize);

    impl<R: AsyncRead + Unpin> AsyncRead for Reads<R> {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            let r = std::pin::Pin::new(&mut self.0).poll_read(cx, buf);
            if r.is_ready() {
                self.1 += 1;
            }
            r
        }
    }

    #[tokio::test]
    async fn a_small_record_is_read_with_one_read_call() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut a, b) = tokio::io::duplex(1 << 16);
        w.send(&mut a, &[7u8; SMALL]).await.unwrap();
        let mut b = Reads(b, 0);
        assert_eq!(r.recv(&mut b).await.unwrap(), Some(&[7u8; SMALL][..]));
        assert_eq!(b.1, 1);
    }

    #[tokio::test]
    async fn data_just_over_the_small_size_goes_as_one_record() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut plain_in, mut plain_src) = tokio::io::duplex(1 << 16);
        let (mut wire_a, mut wire_b) = tokio::io::duplex(1 << 16);
        let data: Vec<u8> = (0..600u32).map(|i| (i % 251) as u8).collect();
        plain_in.write_all(&data).await.unwrap();
        plain_in.shutdown().await.unwrap();
        seal_pump(&mut plain_src, &mut wire_a, &mut w, CHUNK)
            .await
            .unwrap();
        assert_eq!(r.recv(&mut wire_b).await.unwrap(), Some(&data[..]));
        assert_eq!(r.recv(&mut wire_b).await.unwrap(), Some(&[][..]));
    }

    #[tokio::test]
    async fn send_with_seals_every_size_into_one_reused_scratch() {
        let ((mut w, _), (_, mut r)) = pair();
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        let sizes = [MAX_PLAINTEXT, 0, 1, 512, 513, 1400];
        let writer = tokio::spawn(async move {
            let mut scratch = Vec::new();
            for s in sizes {
                let data: Vec<u8> = (0..s).map(|i| (i % 251) as u8).collect();
                w.send_with(&mut a, &data, &mut scratch).await.unwrap();
            }
            let room = scratch.capacity();
            let too_big = vec![0u8; MAX_PLAINTEXT + 1];
            let refused = w.send_with(&mut a, &too_big, &mut scratch).await;
            assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);
            assert_eq!(scratch.capacity(), room);
        });
        for s in sizes {
            let got = r.recv(&mut b).await.unwrap().unwrap().to_vec();
            assert_eq!(got.len(), s);
            assert!(got.iter().enumerate().all(|(i, v)| *v == (i % 251) as u8));
        }
        writer.await.unwrap();
        assert!(r.recv(&mut b).await.unwrap().is_none());
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
