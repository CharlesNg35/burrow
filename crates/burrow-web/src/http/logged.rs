use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, SizeHint};

use super::{Body, BoxError};
use crate::LogFormat;
use crate::log::{Entry, LogFile, line};

pub struct Logged {
    inner: Body,
    started: Instant,
    entry: Option<Entry>,
    sink: Arc<LogFile>,
    format: LogFormat,
}

impl Logged {
    pub fn new(
        inner: Body,
        entry: Entry,
        started: Instant,
        sink: Arc<LogFile>,
        format: LogFormat,
    ) -> Logged {
        Logged {
            inner,
            started,
            entry: Some(entry),
            sink,
            format,
        }
    }

    fn finish(&mut self) {
        if let Some(mut e) = self.entry.take() {
            e.ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.sink.write(line(&e, self.format));
        }
    }
}

impl Drop for Logged {
    fn drop(&mut self) {
        self.finish();
    }
}

impl HttpBody for Logged {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(e), Some(d)) = (this.entry.as_mut(), frame.data_ref()) {
                    e.bytes += u64::try_from(d.len()).unwrap_or(u64::MAX);
                }
            }
            Poll::Ready(None) => this.finish(),
            _ => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::full;
    use http_body_util::BodyExt;
    use std::time::{Duration, SystemTime};

    fn entry() -> Entry {
        Entry {
            time: SystemTime::UNIX_EPOCH,
            client: "203.0.113.9".parse().unwrap(),
            host: "blog.example.com".into(),
            method: "GET".into(),
            target: "/".into(),
            protocol: "HTTP/1.1",
            status: 200,
            bytes: 0,
            ms: 0,
            service: "blog".into(),
            request_id: None,
        }
    }

    fn read(path: &std::path::Path, lines: usize) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let t = std::fs::read_to_string(path).unwrap_or_default();
            if t.lines().count() >= lines || Instant::now() > deadline {
                return t;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[tokio::test]
    async fn one_line_is_written_when_the_body_ends_or_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let sink = Arc::new(LogFile::open(&path).unwrap());
        let body = Logged::new(
            full("hello"),
            entry(),
            Instant::now(),
            sink.clone(),
            LogFormat::Json,
        );
        assert_eq!(body.collect().await.unwrap().to_bytes(), "hello");
        let dropped = Logged::new(
            full("unread"),
            entry(),
            Instant::now(),
            sink,
            LogFormat::Json,
        );
        drop(dropped);
        let text = read(&path, 2);
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["bytes"], 5);
        assert_eq!(lines[1]["bytes"], 0);
    }
}
