use crate::wire::noise::{InMemory, Session};
use crate::wire::record::{RecordReader, RecordWriter};
use crate::wire::transport::Channel;
use crate::wire::transport::Conn;

const PIPE: usize = 32 * 1024;

pub fn pair() -> (Channel, Channel) {
    let (near, far) = tokio::io::duplex(PIPE);
    let session = Session::in_memory(&InMemory(()));
    let channel = |stream| {
        let (sealer, opener) = session.split();
        Channel {
            stream: Conn::Local(stream),
            writer: RecordWriter::new(sealer),
            reader: RecordReader::new(opener),
        }
    };
    (channel(near), channel(far))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_cross_an_in_memory_pair_both_ways() {
        let (mut a, mut b) = pair();
        let big = vec![7u8; crate::wire::noise::MAX_PLAINTEXT];
        a.writer.send(&mut a.stream, b"connect").await.unwrap();
        let sent = tokio::spawn(async move {
            a.writer.send(&mut a.stream, &big).await.unwrap();
            a.writer.send(&mut a.stream, &[]).await.unwrap();
            a
        });
        assert_eq!(
            b.reader.recv(&mut b.stream).await.unwrap(),
            Some(&b"connect"[..])
        );
        let got = b
            .reader
            .recv(&mut b.stream)
            .await
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(got.len(), crate::wire::noise::MAX_PLAINTEXT);
        assert_eq!(b.reader.recv(&mut b.stream).await.unwrap(), Some(&[][..]));
        let mut a = sent.await.unwrap();
        b.writer.send(&mut b.stream, &[0]).await.unwrap();
        assert_eq!(
            a.reader.recv(&mut a.stream).await.unwrap(),
            Some(&[0u8][..])
        );
        assert!(!a.stream.supports_half_close());
        assert!(!a.stream.is_websocket());
    }

    #[tokio::test]
    async fn an_end_dropped_without_its_end_of_stream_is_not_a_clean_end() {
        let (a, mut b) = pair();
        drop(a);
        assert_eq!(b.reader.recv(&mut b.stream).await.unwrap(), None);
    }

    #[test]
    fn in_memory_records_are_refused_by_a_noise_session_and_the_other_way_round() {
        let mut out = [0u8; 64];
        let (mut clear, _) = Session::in_memory(&InMemory(())).split();
        let n = clear.seal(b"hello", &mut out);
        let s = crate::wire::secret::Secret::generate();
        let (init, m1) =
            crate::wire::noise::Initiator::start(burrow_config::Cipher::Chachapoly, &s, b"p");
        let r = crate::wire::noise::Responder::accept(
            burrow_config::Cipher::Chachapoly,
            std::slice::from_ref(&s),
            b"p",
            &m1,
        )
        .unwrap();
        let (srv, m2) = r.reply().unwrap();
        let cli = init.finish(&m2).unwrap();
        let mut plain = [0u8; 64];
        assert!(srv.split().1.open(&out[..n], &mut plain).is_none());
        let sealed = cli.split().0.seal(b"hello", &mut out);
        let (_, mut opener) = Session::in_memory(&InMemory(())).split();
        assert!(opener.open(&out[..sealed], &mut plain).is_none());
    }
}
