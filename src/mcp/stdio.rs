use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::service::ServerInitializeError;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::watch;

use super::tools::Tools;
use crate::base::failure::Failure;

pub const GRACE: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Signal,
    StdinClosed,
}

struct Watched<R> {
    inner: R,
    closed: watch::Sender<bool>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Watched<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let out = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(out, Poll::Ready(Ok(()))) && buf.filled().len() == before && buf.remaining() > 0
        {
            self.closed.send_replace(true);
        }
        out
    }
}

async fn ended(stop: &mut watch::Receiver<bool>, closed: &mut watch::Receiver<bool>) -> Ended {
    tokio::select! {
        _ = stop.wait_for(|s| *s) => Ended::Signal,
        _ = closed.wait_for(|c| *c) => Ended::StdinClosed,
    }
}

pub fn no_app() -> Failure {
    Failure::new("burrow mcp stopped: no AI app spoke to it (its stdin closed before any app sent the MCP handshake)")
        .hint(format!(
            "burrow mcp is meant to be started by an AI app; to add it to one, run: {}",
            crate::base::failure::cmd("mcp --print-config")
        ))
}

fn not_started(e: ServerInitializeError) -> Failure {
    match e {
        ServerInitializeError::ConnectionClosed(_) => no_app(),
        other => Failure::new(format!("cannot start MCP on stdin and stdout: {other}")),
    }
}

pub async fn serve(tools: Tools, mut stop: watch::Receiver<bool>) -> Result<(), Failure> {
    let (closed_tx, mut closed) = watch::channel(false);
    let stdin = Watched {
        inner: tokio::io::stdin(),
        closed: closed_tx,
    };
    let running = tokio::select! {
        biased;
        started = tools.serve((stdin, tokio::io::stdout())) => started.map_err(not_started)?,
        why = ended(&mut stop, &mut closed) => {
            return match why {
                Ended::Signal => Ok(()),
                Ended::StdinClosed => Err(no_app()),
            };
        }
    };
    let cancel = running.cancellation_token();
    let waiting = running.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        done = &mut waiting => {
            return done
                .map(drop)
                .map_err(|e| Failure::new(format!("burrow mcp stopped: {e}")));
        }
        _ = ended(&mut stop, &mut closed) => {}
    }
    cancel.cancel();
    let _ = tokio::time::timeout(GRACE, waiting).await;
    Ok(())
}
