mod common;

use std::path::Path;

use burrow::client::State;
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Loaded, Role, load_file};
use burrow::init::{InitOptions, NoPrompt, Prompter, client_config, init_client, init_server};
use common::*;

fn load(path: &Path) -> Loaded {
    let o = load_file(path, None, &ProcessEnv).unwrap();
    assert!(
        !o.report.has_errors(),
        "{}",
        o.report
            .render_text(burrow::diagnostics::Style { color: false })
    );
    o.loaded.unwrap()
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn server_init_is_valid_private_and_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    for format in [Format::Yaml, Format::Json] {
        let ext = if format == Format::Yaml {
            "yaml"
        } else {
            "json"
        };
        let path = dir.path().join(format!("server.{ext}"));
        let opts = InitOptions {
            format: Some(format),
            output: Some(path.clone()),
            ..Default::default()
        };
        let mut out = Vec::new();
        init_server(&opts, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("burrow client-config home"));
        let o = load_file(&path, None, &ProcessEnv).unwrap();
        assert_eq!(o.role, Some(Role::Server));
        assert_eq!(
            o.report.diagnostics.len(),
            0,
            "{}",
            o.report
                .render_text(burrow::diagnostics::Style { color: false })
        );
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(!contents.contains('#'), "generated files have no comments");

        let before = std::fs::read(&path).unwrap();
        let err = init_server(&opts, &mut Vec::new()).unwrap_err();
        assert!(err.message.contains("already exists"));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "existing file untouched"
        );

        let forced = InitOptions {
            force: true,
            ..opts.clone()
        };
        init_server(&forced, &mut Vec::new()).unwrap();
        assert_ne!(
            std::fs::read(&path).unwrap(),
            before,
            "--force writes a new secret"
        );
    }
}

#[test]
fn env_secret_variant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let opts = InitOptions {
        output: Some(path.clone()),
        env_secret: Some("BURROW_TEST_INIT_SECRET".into()),
        server: Some("example.com:7835".into()),
        ..Default::default()
    };
    let mut out = Vec::new();
    init_server(&opts, &mut out).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("${BURROW_TEST_INIT_SECRET}"));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("export BURROW_TEST_INIT_SECRET="));
    assert!(text.contains("secret: \"${BURROW_TEST_INIT_SECRET}\""));
}

#[test]
fn client_init_needs_flags_without_tty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("client.yaml");
    let opts = InitOptions {
        output: Some(path.clone()),
        server: Some("example.com:7835".into()),
        ..Default::default()
    };
    let err = init_client(&opts, &mut NoPrompt, &mut Vec::new()).unwrap_err();
    assert_eq!(err.exit, 2);
    assert!(err.message.contains("missing client name"));
    assert!(!path.exists());

    let secret = burrow::secret::Secret::generate();
    let full = InitOptions {
        name: Some("home".into()),
        secret: Some(secret.encode()),
        ..opts
    };
    init_client(&full, &mut NoPrompt, &mut Vec::new()).unwrap();
    assert!(matches!(load(&path), Loaded::Client(..)));
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    assert!(init_client(&full, &mut NoPrompt, &mut Vec::new()).is_err());
}

struct Answers(Vec<&'static str>, Vec<String>);

impl Prompter for Answers {
    fn ask(&mut self, q: &str) -> Option<String> {
        self.1.push(q.to_string());
        (!self.0.is_empty()).then(|| self.0.remove(0).to_string())
    }
}

#[test]
fn client_init_prompts_only_for_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("client.yaml");
    let secret = burrow::secret::Secret::generate().encode();
    let leaked: &'static str = Box::leak(secret.into_boxed_str());
    let mut p = Answers(vec!["home", leaked], Vec::new());
    let opts = InitOptions {
        output: Some(path.clone()),
        server: Some("example.com:7835".into()),
        ..Default::default()
    };
    init_client(&opts, &mut p, &mut Vec::new()).unwrap();
    assert_eq!(p.1.len(), 2, "asked only for name and secret: {:?}", p.1);
    assert!(p.1[0].contains("name"));
    assert!(matches!(load(&path), Loaded::Client(..)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printed_client_config_connects_to_generated_server() {
    let dir = tempfile::tempdir().unwrap();
    let server_path = dir.path().join("server.yaml");
    init_server(
        &InitOptions {
            output: Some(server_path.clone()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();

    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    let svc_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    drop(free);
    let echo = echo_server().await;
    let text = std::fs::read_to_string(&server_path)
        .unwrap()
        .replace("0.0.0.0:7835", &format!("127.0.0.1:{port}"))
        .replace("0.0.0.0:7840", &format!("127.0.0.1:{svc_port}"))
        .replace("127.0.0.1:3000", &echo.to_string());
    std::fs::write(&server_path, text).unwrap();
    let Loaded::Server(server_settings, _) = load(&server_path) else {
        panic!()
    };

    let out = client_config(
        "home",
        &server_path,
        &format!("127.0.0.1:{port}"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap();
    let client_path = dir.path().join("client.yaml");
    std::fs::write(&client_path, &out.text).unwrap();
    let Loaded::Client(client_settings, _) = load(&client_path) else {
        panic!()
    };
    assert!(out.text.contains(&format!("127.0.0.1/32:{}", echo.port())));

    let srv = start_server(*server_settings).await;
    let mut cli = start_client(*client_settings).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    assert_eq!(
        roundtrip(srv.services["web"], b"from init").await,
        b"from init"
    );
}

#[test]
fn client_config_unknown_name_suggests() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();
    let err = match client_config("hme", &path, "a.b:1", Format::Yaml, &ProcessEnv) {
        Err(e) => e,
        Ok(_) => panic!("unknown client must fail"),
    };
    assert!(err.message.contains("no client named \"hme\""));
    assert!(err.hint.unwrap().contains("did you mean \"home\""));
}

#[test]
fn websocket_init_writes_only_what_was_asked() {
    use burrow::init::WebSocketInit;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let mut out = Vec::new();
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            server: Some("tunnel.example.com".into()),
            websocket: Some(WebSocketInit::default()),
            ..Default::default()
        },
        &mut out,
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("listen: 0.0.0.0"),
        "no TCP control port: {text}"
    );
    assert!(
        text.contains("websocket:\n  listen: 127.0.0.1:7836\n  path: /burrow\n"),
        "{text}"
    );
    assert!(!text.contains("tls"), "{text}");
    let Loaded::Server(settings, _) = load(&path) else {
        panic!()
    };
    assert!(settings.listen.is_none());
    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("server: wss://tunnel.example.com/burrow"),
        "{printed}"
    );

    let tls_path = dir.path().join("tls.yaml");
    let mut out = Vec::new();
    init_server(
        &InitOptions {
            output: Some(tls_path.clone()),
            server: Some("tunnel.example.com".into()),
            websocket: Some(WebSocketInit {
                tls_cert: Some("cert.pem".into()),
                tls_key: Some("key.pem".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &mut out,
    )
    .unwrap();
    let text = std::fs::read_to_string(&tls_path).unwrap();
    assert!(
        text.contains("listen: 0.0.0.0:7837") && text.contains("cert: cert.pem"),
        "{text}"
    );
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("server: wss://tunnel.example.com:7837/burrow")
    );

    let half = init_server(
        &InitOptions {
            output: Some(dir.path().join("half.yaml")),
            websocket: Some(WebSocketInit {
                tls_cert: Some("cert.pem".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(half.exit, 2);
    assert!(!dir.path().join("half.yaml").exists());
}

#[test]
fn plain_init_has_no_websocket_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("listen: 0.0.0.0:7835\n"), "{text}");
    assert!(!text.contains("websocket") && !text.contains("reconnect") && !text.contains("tls"));
}

#[test]
fn client_config_matches_the_server_transport() {
    use burrow::init::WebSocketInit;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws.yaml");
    init_server(
        &InitOptions {
            output: Some(ws.clone()),
            websocket: Some(WebSocketInit::default()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();
    let err = client_config(
        "home",
        &ws,
        "tunnel.example.com:7835",
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(err.message.contains("only accepts WebSocket clients"));
    let ok = client_config(
        "home",
        &ws,
        "wss://tunnel.example.com/burrow",
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap();
    assert!(ok.text.contains("server: wss://tunnel.example.com/burrow"));

    let tcp = dir.path().join("tcp.yaml");
    init_server(
        &InitOptions {
            output: Some(tcp.clone()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();
    let err = client_config(
        "home",
        &tcp,
        "wss://tunnel.example.com/burrow",
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(err.message.contains("has no websocket section"));
}
