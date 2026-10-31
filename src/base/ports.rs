use std::future::Future;
use std::time::Duration;

use burrow_config::Protocol;

pub(crate) const RETRY_GAP: Duration = Duration::from_millis(100);

pub(crate) async fn retry<T, E, F, Fut>(tries: usize, mut attempt: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut tries = tries.max(1);
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if tries <= 1 => return Err(e),
            Err(_) => tries -= 1,
        }
        tokio::time::sleep(RETRY_GAP).await;
    }
}

pub(crate) fn inspect_hint(proto: Protocol, port: u16) -> String {
    if cfg!(windows) {
        format!("netstat -ano | findstr :{port}")
    } else if cfg!(target_os = "linux") {
        let flag = if proto == Protocol::Udp {
            "-lunp"
        } else {
            "-ltnp"
        };
        format!("sudo ss {flag} 'sport = :{port}'")
    } else if cfg!(target_os = "freebsd") {
        let flag = proto.wire();
        format!("sockstat -46 -l -P {flag} -p {port}")
    } else {
        let flag = if proto == Protocol::Udp { "UDP" } else { "TCP" };
        format!("sudo lsof -nP -i{flag}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_bind_is_tried_the_given_number_of_times_then_its_last_error_is_kept() {
        let mut calls = 0;
        let r: Result<(), usize> = retry(3, || {
            calls += 1;
            let n = calls;
            async move { Err(n) }
        })
        .await;
        assert_eq!(r, Err(3));
        let mut calls = 0;
        let r: Result<usize, usize> = retry(5, || {
            calls += 1;
            let n = calls;
            async move { if n == 2 { Ok(n) } else { Err(n) } }
        })
        .await;
        assert_eq!(r, Ok(2));
        let r: Result<(), &str> = retry(0, || async { Err("once") }).await;
        assert_eq!(r, Err("once"));
    }
}
