use std::net::SocketAddr;

use burrow::config::settings::{ServerSettings, WebSettings};
use burrow::server::Server;
use burrow::wire::secret::Secret;

use crate::common::*;

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn with_web(https: Option<SocketAddr>, http: Option<SocketAddr>) -> ServerSettings {
    let mut s = server_settings(vec![Secret::generate()]);
    s.web = Some(WebSettings::on((https, http)));
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_refuses_a_web_port_another_program_holds_with_the_find_hint() {
    let held = std::net::TcpListener::bind(local(0)).unwrap();
    let taken = held.local_addr().unwrap();
    for s in [with_web(Some(taken), None), with_web(None, Some(taken))] {
        let Err(f) = Server::bind(s).await else {
            panic!("the server started on a taken port");
        };
        assert!(
            f.message.contains("address already in use"),
            "{}",
            f.message
        );
        assert!(f.message.contains(&taken.to_string()), "{}", f.message);
        let hint = f.hint.unwrap_or_default();
        assert!(hint.contains(&taken.port().to_string()), "{hint}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reload_to_a_taken_web_port_keeps_the_old_one_says_so_and_moves_once_it_is_free() {
    let now = local(free_port());
    let srv = start_server(with_web(Some(now), None)).await;
    let held = std::net::TcpListener::bind(local(0)).unwrap();
    let taken = held.local_addr().unwrap();
    let mut applies = srv.shared.applies();
    let seq = applies.borrow().seq;
    let mut next = srv.shared.settings_snapshot();
    next.web = Some(WebSettings::on((Some(taken), None)));
    srv.reload(next).await;
    let report = applies.wait_for(|r| r.seq > seq).await.unwrap().clone();
    assert!(
        report.unbound.iter().any(|u| u == "web.https"),
        "{report:?}"
    );
    let line = format!(
        "web.https {taken} is in use; the web proxy stays on https {now} and tries again every"
    );
    assert!(
        report.lines.iter().any(|l| l.starts_with(&line)),
        "{:?}",
        report.lines
    );
    let status = srv.shared.status();
    let wait = status.web_waiting.expect("status says the web proxy waits");
    assert_eq!(
        (wait.key.as_str(), wait.reason.as_str()),
        ("web.https", "in use")
    );
    assert_eq!(wait.addr, taken.to_string());
    assert!(
        std::net::TcpStream::connect(now).is_ok(),
        "the old port still serves"
    );
    drop(held);
    let shared = srv.shared.clone();
    wait_long(|| shared.status().web_waiting.is_none()).await;
    assert!(
        std::net::TcpStream::connect(taken).is_ok(),
        "it moved once the port was free"
    );
}
