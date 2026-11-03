use tokio::sync::watch;

use crate::base::failure::Failure;

fn runtime() -> Result<tokio::runtime::Runtime, Failure> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::new(format!("cannot start the async runtime: {e}")))
}

pub(super) fn hold_hangup() {
    #[cfg(unix)]
    let _ = signal_hook::flag::register(
        signal_hook::consts::SIGHUP,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
}

pub(super) fn drive<T>(
    work: impl std::future::Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    drive_for(work, std::time::Duration::from_secs(1))
}

pub(super) fn drive_for<T>(
    work: impl std::future::Future<Output = Result<T, Failure>>,
    linger: std::time::Duration,
) -> Result<T, Failure> {
    let rt = runtime()?;
    #[cfg(unix)]
    rt.block_on(async {
        let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup());
    });
    let out = rt.block_on(work);
    rt.shutdown_timeout(linger);
    out
}

#[cfg(unix)]
async fn terminate() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(not(unix))]
async fn terminate() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

pub(super) fn stop_on_signal(external: Option<watch::Receiver<bool>>) -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        match external {
            Some(mut ext) => {
                tokio::select! {
                    _ = terminate() => {}
                    _ = ext.wait_for(|stop| *stop) => {}
                }
            }
            None => terminate().await,
        }
        let _ = tx.send(true);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_does_not_wait_for_a_stuck_blocking_lookup() {
        let started = std::time::Instant::now();
        let out = drive(async {
            tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_secs(20)));
            Ok(7)
        });
        assert_eq!(out.unwrap(), 7);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
