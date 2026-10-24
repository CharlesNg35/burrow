use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use burrow_admin_proto::frame::AdminOpen;

use super::Shared;
use crate::wire::protocol::Status as Answer;
use crate::wire::secret::Secret;
use crate::wire::transport::Channel;

pub struct Opened {
    pub visitor: String,
    pub secret: Secret,
    pub peer: SocketAddr,
    pub via_websocket: bool,
}

pub trait AdminHook: Send + Sync {
    fn serve(
        self: Arc<Self>,
        shared: Arc<Shared>,
        channel: Channel,
        open: AdminOpen,
        opened: Opened,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

pub(crate) async fn answer(ch: &mut Channel, records: &[&[u8]]) {
    use tokio::io::AsyncWriteExt;
    let sent = async {
        for r in records {
            ch.writer.send(&mut ch.stream, r).await?;
        }
        ch.writer.send(&mut ch.stream, &[]).await?;
        ch.stream.shutdown().await
    };
    let _ = tokio::time::timeout(crate::forward::ANSWER_TIMEOUT, sent).await;
}

pub(super) async fn serve(shared: Arc<Shared>, mut channel: Channel, first: &[u8], opened: Opened) {
    let Ok(open) = AdminOpen::decode(first) else {
        answer(&mut channel, &[&[Answer::Invalid as u8]]).await;
        return;
    };
    let Some(hook) = shared.admin() else {
        answer(
            &mut channel,
            &[
                &[Answer::Invalid as u8],
                b"this server runs without a config file, so it has nothing to administer",
            ],
        )
        .await;
        return;
    };
    hook.serve(shared, channel, open, opened).await;
}
