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
        assert!(text.contains("burrow client-config -c "), "{text}");
        assert!(
            text.contains("--server YOUR_PUBLIC_HOST -o client.yaml"),
            "{text}"
        );
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
        Some("home"),
        &server_path,
        Some(&format!("127.0.0.1:{port}")),
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
    let err = match client_config(Some("hme"), &path, Some("a.b:1"), Format::Yaml, &ProcessEnv) {
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
        text.contains("listen: 0.0.0.0:7837")
            && text.contains(&format!(
                "cert: {}",
                std::path::absolute("cert.pem").unwrap().display()
            )),
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
        Some("home"),
        &ws,
        Some("tunnel.example.com:7835"),
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(err.message.contains("only accepts WebSocket clients"));
    let ok = client_config(
        Some("home"),
        &ws,
        Some("wss://tunnel.example.com/burrow"),
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
        Some("home"),
        &tcp,
        Some("wss://tunnel.example.com/burrow"),
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(err.message.contains("has no websocket section"));
}

fn server_with(dir: &Path, top: &str, clients: &[&str]) -> std::path::PathBuf {
    let path = dir.join("server.yaml");
    let mut text = format!("{top}\nclients:\n");
    for c in clients {
        text.push_str(&format!(
            "  {c}:\n    secret: {}\n",
            burrow::secret::Secret::generate().encode()
        ));
    }
    text.push_str(&format!(
        "services:\n  web:\n    client: {}\n    bind: 7840\n    target: 3000\n",
        clients[0]
    ));
    std::fs::write(&path, text).unwrap();
    path
}

fn server_line(text: &str) -> &str {
    text.lines().next().unwrap()
}

fn generate(
    path: &Path,
    server: Option<&str>,
) -> Result<burrow::init::ClientConfigOutput, burrow::failure::Failure> {
    client_config(None, path, server, Format::Yaml, &ProcessEnv)
}

#[test]
fn client_config_needs_server_or_public_address() {
    let dir = tempfile::tempdir().unwrap();
    for listen in [
        "0.0.0.0:7835",
        "203.0.113.5:7835",
        "192.168.1.10:7835",
        "127.0.0.1:7835",
    ] {
        let path = server_with(dir.path(), &format!("listen: \"{listen}\""), &["home"]);
        let err = generate(&path, None).err().unwrap();
        assert_eq!(
            err.message,
            "client-config needs the address clients connect to"
        );
        let hint = err.hint.unwrap();
        assert!(hint.contains("--server myserver.example.com"), "{hint}");
        assert!(
            hint.contains("\"public_address: myserver.example.com\""),
            "{hint}"
        );
        assert!(hint.contains("the port, 7835, comes from listen"), "{hint}");
    }
}

#[test]
fn client_config_uses_public_address() {
    let dir = tempfile::tempdir().unwrap();
    let path = server_with(
        dir.path(),
        "listen: 0.0.0.0:2333\npublic_address: myserver.example.com",
        &["home"],
    );
    let out = generate(&path, None).unwrap();
    assert_eq!(server_line(&out.text), "server: myserver.example.com:2333");
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    let out = generate(&path, Some("other.example.com")).unwrap();
    assert_eq!(server_line(&out.text), "server: other.example.com:2333");
}

#[test]
fn client_config_takes_the_port_from_listen() {
    let dir = tempfile::tempdir().unwrap();
    let path = server_with(dir.path(), "listen: 0.0.0.0:2333", &["home"]);
    for (given, want) in [
        ("myserver.example.com", "server: myserver.example.com:2333"),
        ("203.0.113.5", "server: 203.0.113.5:2333"),
        ("[2001:db8::1]", "server: \"[2001:db8::1]:2333\""),
    ] {
        let out = generate(&path, Some(given)).unwrap();
        assert_eq!(server_line(&out.text), want);
        assert!(out.notes.is_empty(), "{:?}", out.notes);
        assert!(out.text.contains("127.0.0.1/32:3000"), "{}", out.text);
    }
    let out = generate(&path, Some("myserver.example.com:9000")).unwrap();
    assert_eq!(server_line(&out.text), "server: myserver.example.com:9000");
    assert!(
        out.notes[0].contains("port 9000 differs from the listen port 2333"),
        "{:?}",
        out.notes
    );
}

#[test]
fn client_config_notes_and_refuses_unreachable_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let path = server_with(dir.path(), "listen: 0.0.0.0:7835", &["home"]);
    assert!(
        generate(&path, Some("localhost")).unwrap().notes[0].contains("only clients running on it")
    );
    assert!(
        generate(&path, Some("127.0.0.1")).unwrap().notes[0].contains("only clients running on it")
    );
    assert!(generate(&path, Some("10.0.0.5")).unwrap().notes[0].contains("same network or VPN"));
    for bad in ["0.0.0.0", "[::]", "myserver.com:", "a b"] {
        let err = generate(&path, Some(bad)).err().unwrap();
        assert!(
            err.message.starts_with("invalid --server"),
            "{bad}: {}",
            err.message
        );
    }
}

#[test]
fn client_config_name_is_optional_with_one_client() {
    let dir = tempfile::tempdir().unwrap();
    let one = server_with(dir.path(), "listen: 0.0.0.0:7835", &["home"]);
    let out = client_config(None, &one, Some("h.example.com"), Format::Yaml, &ProcessEnv).unwrap();
    assert!(out.text.contains("name: home"));

    let two = server_with(dir.path(), "listen: 0.0.0.0:7835", &["home", "office"]);
    let err = client_config(None, &two, Some("h.example.com"), Format::Yaml, &ProcessEnv)
        .err()
        .unwrap();
    assert!(
        err.message.contains("defines 2 clients; which one?"),
        "{}",
        err.message
    );
    assert!(err.hint.unwrap().contains("defined clients: home, office"));
    let out = client_config(
        Some("office"),
        &two,
        Some("h.example.com"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap();
    assert!(out.text.contains("name: office"));
}

#[test]
fn client_config_builds_the_websocket_url_from_a_host() {
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
    let out = client_config(
        None,
        &ws,
        Some("tunnel.example.com"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap();
    assert!(
        out.text.contains("server: wss://tunnel.example.com/burrow"),
        "{}",
        out.text
    );
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    let err = client_config(None, &ws, None, Format::Yaml, &ProcessEnv)
        .err()
        .unwrap();
    assert!(err.message.contains("needs the address"), "{}", err.message);
    let err = client_config(
        None,
        &ws,
        Some("tunnel.example.com:7835"),
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(
        err.message.contains("only accepts WebSocket clients"),
        "{}",
        err.message
    );
}

#[test]
fn init_accepts_a_host_without_port() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let mut out = Vec::new();
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            server: Some("myserver.example.com".into()),
            ..Default::default()
        },
        &mut out,
    )
    .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("server: myserver.example.com:7835"), "{text}");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.starts_with("listen: 0.0.0.0:7835\npublic_address: myserver.example.com\n"),
        "{written}"
    );
    assert!(matches!(load(&path), Loaded::Server(..)));
    let again = client_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    assert!(
        again
            .text
            .starts_with("server: myserver.example.com:7835\n"),
        "{}",
        again.text
    );

    let client = dir.path().join("client.yaml");
    init_client(
        &InitOptions {
            output: Some(client.clone()),
            server: Some("myserver.example.com".into()),
            name: Some("home".into()),
            secret: Some(burrow::secret::Secret::generate().encode()),
            ..Default::default()
        },
        &mut NoPrompt,
        &mut Vec::new(),
    )
    .unwrap();
    let text = std::fs::read_to_string(&client).unwrap();
    assert!(
        text.starts_with("server: myserver.example.com:7835\n"),
        "{text}"
    );
    assert!(text.contains("allow:"), "{text}");
    assert!(matches!(load(&client), Loaded::Client(..)));
}

#[test]
fn init_takes_the_format_from_the_output_extension() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.json");
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().starts_with('{'));
    let err = init_server(
        &InitOptions {
            output: Some(dir.path().join("other.json")),
            format: Some(Format::Yaml),
            ..Default::default()
        },
        &mut Vec::new(),
    )
    .err()
    .unwrap();
    assert!(err.message.contains("does not match --format"));
}

#[test]
fn output_format_follows_the_extension() {
    use burrow::init::output_format;
    assert_eq!(output_format(None, None).unwrap(), Format::Yaml);
    assert_eq!(
        output_format(Some(Format::Json), None).unwrap(),
        Format::Json
    );
    assert_eq!(
        output_format(None, Some(Path::new("c.json"))).unwrap(),
        Format::Json
    );
    assert_eq!(
        output_format(None, Some(Path::new("c.yml"))).unwrap(),
        Format::Yaml
    );
    assert!(output_format(Some(Format::Yaml), Some(Path::new("c.json"))).is_err());
    assert!(output_format(None, Some(Path::new("client"))).is_err());
}

#[test]
fn init_refuses_server_addresses_that_validate_would_reject() {
    use burrow::init::WebSocketInit;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.yaml");
    let run = |server: &str, websocket: Option<WebSocketInit>| {
        init_server(
            &InitOptions {
                output: Some(path.clone()),
                force: true,
                server: Some(server.into()),
                websocket,
                ..Default::default()
            },
            &mut Vec::new(),
        )
    };
    assert!(run("0.0.0.0", None).is_err());
    assert!(run("myhost:8443", Some(WebSocketInit::default())).is_err());
    for (server, ws) in [
        ("myhost", None),
        ("myhost:9000", None),
        ("myhost", Some(WebSocketInit::default())),
        ("wss://myhost:8443/burrow", Some(WebSocketInit::default())),
    ] {
        run(server, ws).unwrap();
        assert!(matches!(load(&path), Loaded::Server(..)), "{server}");
    }
}

#[test]
fn init_and_client_config_agree_on_the_tls_port_for_ipv6() {
    use burrow::init::WebSocketInit;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.yaml");
    let mut out = Vec::new();
    init_server(
        &InitOptions {
            output: Some(path.clone()),
            server: Some("[2001:db8::1]".into()),
            websocket: Some(WebSocketInit {
                tls_cert: Some(dir.path().join("c.pem").display().to_string()),
                tls_key: Some(dir.path().join("k.pem").display().to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        &mut out,
    )
    .unwrap();
    let printed = String::from_utf8(out).unwrap();
    assert!(
        printed.contains("wss://[2001:db8::1]:7837/burrow"),
        "{printed}"
    );
}
