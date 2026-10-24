use burrow::init::{InitOptions, WebSocketInit, init_server};

use super::load;

fn tunnel(server: &str, ws: WebSocketInit) -> (tempfile::TempDir, Result<String, u8>, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let mut out = Vec::new();
    let got = init_server(
        &InitOptions {
            output: Some(path.clone()),
            server: Some(server.into()),
            websocket: Some(ws),
            email: Some("you@example.com".into()),
            ..Default::default()
        },
        &mut out,
    );
    let text = got
        .map(|_| std::fs::read_to_string(&path).unwrap())
        .map_err(|e| e.exit);
    (dir, text, String::from_utf8(out).unwrap())
}

#[test]
fn ws_with_an_email_writes_the_tunnel_only_web_block() {
    let (dir, text, printed) = tunnel("tunnel.example.com", WebSocketInit::default());
    let text = text.unwrap();
    assert!(
        text.contains(
            "web:\n  https: 443\n  letsencrypt:\n    email: you@example.com\nwebsocket:\n  listen: 443\n  host: tunnel.example.com\n  path: /burrow\n"
        ),
        "{text}"
    );
    assert!(!text.contains("public_address"), "{text}");
    load(&dir.path().join("server.yaml"));
    assert!(
        printed.contains("server: wss://tunnel.example.com/burrow"),
        "{printed}"
    );
    assert!(printed.contains("Let's Encrypt"), "{printed}");
}

#[test]
fn ws_with_an_email_needs_a_plain_name_and_no_own_port_or_files() {
    for (server, ws) in [
        ("wss://tunnel.example.com/burrow", WebSocketInit::default()),
        (
            "tunnel.example.com",
            WebSocketInit {
                listen: Some("0.0.0.0:7837".into()),
                ..Default::default()
            },
        ),
        (
            "tunnel.example.com",
            WebSocketInit {
                tls_cert: Some("c.pem".into()),
                tls_key: Some("k.pem".into()),
                ..Default::default()
            },
        ),
    ] {
        let (dir, text, _) = tunnel(server, ws);
        assert_eq!(text.err(), Some(2), "{server}");
        assert!(!dir.path().join("server.yaml").exists());
    }
}

#[test]
fn an_email_alone_says_what_it_is_for() {
    let dir = tempfile::tempdir().unwrap();
    let e = init_server(
        &InitOptions {
            output: Some(dir.path().join("server.yaml")),
            email: Some("you@example.com".into()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(e.exit, 2);
    assert!(e.message.contains("--email"), "{}", e.message);
}
