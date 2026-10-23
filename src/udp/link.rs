use std::time::Duration;

use crate::handshake::Channel;
use crate::protocol::Status;
use crate::record::{RecordReader, RecordWriter};
use crate::transport::{OwnedReadHalf, OwnedWriteHalf};

pub struct Link {
    pub reader: RecordReader,
    pub rd: OwnedReadHalf,
    pub writer: RecordWriter,
    pub wr: OwnedWriteHalf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    Lost,
    Silent,
    Refused(Status),
}

pub async fn open_udp(ch: Channel, request: &[u8], wait: Duration) -> Result<Link, Opening> {
    let Channel {
        stream,
        mut writer,
        mut reader,
    } = ch;
    let (mut rd, mut wr) = stream.into_split();
    writer
        .send(&mut wr, request)
        .await
        .map_err(|_| Opening::Lost)?;
    let status = match tokio::time::timeout(wait, reader.recv(&mut rd)).await {
        Ok(Ok(Some(b))) => Status::decode(b).map_err(|_| Opening::Lost)?,
        Ok(_) => return Err(Opening::Lost),
        Err(_) => return Err(Opening::Silent),
    };
    if status != Status::Ok {
        return Err(Opening::Refused(status));
    }
    Ok(Link {
        reader,
        rd,
        writer,
        wr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::local::pair;

    const WAIT: Duration = Duration::from_secs(5);

    async fn answered(answer: Option<&'static [u8]>, wait: Duration) -> Result<Link, Opening> {
        let (near, mut far) = pair();
        let peer = tokio::spawn(async move {
            let got = far
                .reader
                .recv(&mut far.stream)
                .await
                .unwrap()
                .unwrap()
                .to_vec();
            assert_eq!(got, b"connect");
            match answer {
                Some(a) => far.writer.send(&mut far.stream, a).await.unwrap(),
                None => tokio::time::sleep(Duration::from_secs(60)).await,
            }
            far
        });
        let opened = open_udp(near, b"connect", wait).await;
        peer.abort();
        opened
    }

    #[tokio::test]
    async fn an_ok_status_hands_over_the_link() {
        let (near, mut far) = pair();
        let peer = tokio::spawn(async move {
            far.reader.recv(&mut far.stream).await.unwrap().unwrap();
            far.writer.send(&mut far.stream, &[0]).await.unwrap();
            far.reader
                .recv(&mut far.stream)
                .await
                .unwrap()
                .unwrap()
                .to_vec()
        });
        let mut link = open_udp(near, b"connect", WAIT).await.ok().unwrap();
        link.writer.send(&mut link.wr, b"after").await.unwrap();
        assert_eq!(peer.await.unwrap(), b"after");
    }

    #[tokio::test]
    async fn a_refusal_carries_its_status() {
        assert_eq!(
            answered(Some(&[7]), WAIT).await.err(),
            Some(Opening::Refused(Status::Busy))
        );
    }

    #[tokio::test]
    async fn an_unreadable_status_is_a_lost_channel() {
        assert_eq!(
            answered(Some(&[0, 0]), WAIT).await.err(),
            Some(Opening::Lost)
        );
        assert_eq!(answered(Some(&[99]), WAIT).await.err(), Some(Opening::Lost));
    }

    #[tokio::test]
    async fn no_answer_in_time_is_silence() {
        assert_eq!(
            answered(None, Duration::from_millis(50)).await.err(),
            Some(Opening::Silent)
        );
    }

    #[tokio::test]
    async fn a_closed_channel_is_lost() {
        let (near, far) = pair();
        drop(far);
        assert_eq!(
            open_udp(near, b"connect", WAIT).await.err(),
            Some(Opening::Lost)
        );
    }
}
