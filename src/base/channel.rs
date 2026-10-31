use tokio::sync::mpsc;

pub async fn next_reload<T>(reloads: &mut Option<mpsc::Receiver<T>>) -> T {
    loop {
        match reloads {
            Some(rx) => match rx.recv().await {
                Some(next) => return next,
                None => *reloads = None,
            },
            None => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn a_reload_arrives_and_a_closed_channel_waits_forever() {
        let (tx, rx) = mpsc::channel(1);
        let mut reloads = Some(rx);
        tx.send(7).await.unwrap();
        assert_eq!(next_reload(&mut reloads).await, 7);
        drop(tx);
        let waited = tokio::time::timeout(Duration::from_millis(50), next_reload(&mut reloads));
        assert!(waited.await.is_err());
        assert!(reloads.is_none());
    }
}
