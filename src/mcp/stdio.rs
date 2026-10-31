use rmcp::ServiceExt;
use tokio::sync::watch;

use super::tools::Tools;
use crate::base::failure::Failure;

pub async fn serve(tools: Tools, mut stop: watch::Receiver<bool>) -> Result<(), Failure> {
    let running = tools
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| Failure::new(format!("cannot start MCP on stdin and stdout: {e}")))?;
    let cancel = running.cancellation_token();
    let watcher = tokio::spawn(async move {
        if stop.wait_for(|s| *s).await.is_ok() {
            cancel.cancel();
        }
    });
    let ended = running.waiting().await;
    watcher.abort();
    ended
        .map(drop)
        .map_err(|e| Failure::new(format!("burrow mcp stopped: {e}")))
}
