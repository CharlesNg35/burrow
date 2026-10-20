use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::allowlist::Gate;
use crate::client::data::{Dialing, Dials, Rules, serve_local};
use crate::config::settings::ServerSettings;
use crate::handshake::Channel;
use crate::protocol::TOKEN_LEN;
use crate::transport::resolve::Resolver;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

impl Dials for Arc<Dialing> {
    fn dialing(&self) -> Arc<Dialing> {
        self.clone()
    }
}

pub(crate) struct SelfDialer {
    rules: Arc<watch::Sender<Arc<Dialing>>>,
    token: Mutex<[u8; TOKEN_LEN]>,
    resolver: Arc<dyn Resolver>,
}

impl SelfDialer {
    pub(crate) fn new(
        settings: &ServerSettings,
        own: Vec<SocketAddr>,
        resolver: Arc<dyn Resolver>,
    ) -> SelfDialer {
        SelfDialer {
            rules: Arc::new(watch::Sender::new(Arc::new(dialing(settings, own)))),
            token: Mutex::new(fresh_token()),
            resolver,
        }
    }

    pub(crate) fn reconfigure(&self, settings: &ServerSettings, own: Vec<SocketAddr>) {
        let next = dialing(settings, own);
        let current = self.rules.borrow().clone();
        if *current == next {
            return;
        }
        if current.gate != next.gate {
            *self.token.lock().expect("token lock") = fresh_token();
        }
        self.rules.send_replace(Arc::new(next));
    }

    pub(crate) fn open(&self) -> Channel {
        let (near, far) = crate::transport::local::pair();
        tokio::spawn(serve_local(
            Rules(self.rules.clone()),
            self.resolver.clone(),
            far,
        ));
        near
    }

    pub(crate) fn token(&self) -> [u8; TOKEN_LEN] {
        *self.token.lock().expect("token lock")
    }

    pub(crate) fn allow(&self) -> Vec<String> {
        self.rules.borrow().gate.allow().describe()
    }
}

fn dialing(settings: &ServerSettings, own: Vec<SocketAddr>) -> Dialing {
    Dialing {
        gate: Gate::this_server(settings.self_allow.as_ref(), own),
        connect_timeout: CONNECT_TIMEOUT,
        tcp: settings.tcp.clone(),
        udp_session_timeout: settings.udp_session_timeout,
        plaintext: false,
        list: "\"self.allow\" in the server config",
    }
}

fn fresh_token() -> [u8; TOKEN_LEN] {
    let mut token = [0u8; TOKEN_LEN];
    getrandom::fill(&mut token).expect("operating system random source is available");
    token
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::Protocol;
    use crate::protocol::{Connect, Status};

    fn settings(allow: &[&str]) -> ServerSettings {
        let yaml = format!(
            "listen: 127.0.0.1:7835\nself:\n  allow: [{}]\nclients:\n  home:\n    secret: {}\n",
            allow.join(", "),
            crate::secret::Secret::generate().encode()
        );
        match crate::config::load_str(
            "server.yaml",
            &yaml,
            crate::config::Format::Yaml,
            Some(crate::config::Role::Server),
            &|_: &str| None,
        )
        .loaded
        {
            Some(crate::config::Loaded::Server(s, _)) => *s,
            _ => panic!("server config loads"),
        }
    }

    async fn ask(dialer: &SelfDialer, target: String) -> Status {
        let mut ch = dialer.open();
        let connect = Connect {
            protocol: Protocol::Tcp,
            service: "web".into(),
            target,
            only: None,
        };
        ch.writer
            .send(&mut ch.stream, &connect.encode())
            .await
            .unwrap();
        let reply = ch.reader.recv(&mut ch.stream).await.unwrap().unwrap();
        Status::decode(reply).unwrap()
    }

    #[tokio::test]
    async fn a_local_channel_dials_inside_self_allow_and_refuses_outside_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let at = listener.local_addr().unwrap();
        let own: SocketAddr = "127.0.0.1:7835".parse().unwrap();
        let dialer = SelfDialer::new(
            &settings(&["127.0.0.0/8"]),
            vec![own],
            crate::transport::resolve::system(),
        );
        assert_eq!(ask(&dialer, at.to_string()).await, Status::Ok);
        assert_eq!(ask(&dialer, "10.0.0.1:22".into()).await, Status::Denied);
        assert_eq!(ask(&dialer, own.to_string()).await, Status::Denied);
        assert_eq!(
            ask(&dialer, "169.254.169.254:80".into()).await,
            Status::Denied
        );
    }

    #[test]
    fn the_token_is_renewed_only_when_what_self_may_reach_changes() {
        let narrow = settings(&["127.0.0.1/32"]);
        let dialer = SelfDialer::new(&narrow, Vec::new(), crate::transport::resolve::system());
        let first = dialer.token();
        dialer.reconfigure(&narrow, Vec::new());
        assert_eq!(dialer.token(), first);
        let mut slower = narrow.clone();
        slower.udp_session_timeout = Duration::from_secs(5);
        dialer.reconfigure(&slower, Vec::new());
        assert_eq!(dialer.token(), first);
        dialer.reconfigure(&settings(&["127.0.0.0/8"]), Vec::new());
        assert_ne!(dialer.token(), first);
        assert_eq!(dialer.allow(), vec!["127.0.0.0/8"]);
    }
}
