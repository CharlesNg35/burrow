use std::net::IpAddr;

use burrow_syntax::share::{Share, Shares};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Pool {
    Service(String),
    Client(String),
}

pub(super) type FairShare = Share<(Pool, IpAddr)>;

#[derive(Default)]
pub(super) struct VisitorShares(Shares<(Pool, IpAddr)>);

impl VisitorShares {
    pub(super) fn take(
        &self,
        pool: Pool,
        source: Option<IpAddr>,
        total: usize,
        used: usize,
    ) -> Result<Option<FairShare>, ()> {
        let Some(source) = source else {
            return Ok(None);
        };
        self.0.take((pool, source), total, used).map(Some).ok_or(())
    }
}

#[cfg(feature = "web")]
pub(super) fn web_source(peer: std::net::SocketAddr) -> Option<IpAddr> {
    burrow_syntax::share::source_key(peer.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        burrow_syntax::share::source_key(s.parse().unwrap())
    }

    #[test]
    fn a_service_and_a_client_keep_separate_parts_for_one_address() {
        let shares = VisitorShares::default();
        let a = ip("203.0.113.7");
        let service = || Pool::Service("app".into());
        let held: Vec<_> = (0..8)
            .map(|_| shares.take(service(), a, 16, 15).unwrap())
            .collect();
        assert!(shares.take(service(), a, 16, 15).is_err());
        assert!(shares.take(Pool::Service("blog".into()), a, 16, 15).is_ok());
        assert!(shares.take(Pool::Client("home".into()), a, 16, 15).is_ok());
        assert!(shares.take(service(), ip("203.0.113.8"), 16, 15).is_ok());
        assert!(matches!(
            shares.take(service(), ip("127.0.0.1"), 16, 16),
            Ok(None)
        ));
        drop(held);
        assert!(shares.take(service(), a, 16, 15).is_ok());
    }
}
