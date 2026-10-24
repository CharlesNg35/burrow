mod common;

use std::path::Path;

use burrow::base::prompt::{NoPrompt, Prompter};
use burrow::client::State;
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Loaded, Role, load_file};
use burrow::init::{InitOptions, client_config, init_client, init_server};
use common::*;

fn load(path: &Path) -> Loaded {
    let o = load_file(path, None, &ProcessEnv).unwrap();
    assert!(
        !o.report.has_errors(),
        "{}",
        o.report
            .render_text(burrow::base::style::Style { color: false })
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
                .render_text(burrow::base::style::Style { color: false })
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

    let secret = burrow::wire::secret::Secret::generate();
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
    let secret = burrow::wire::secret::Secret::generate().encode();
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

    let port = common::free_port();
    let svc_port = common::free_port();
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
    let root = burrow::config::tree::parse(&text).unwrap();
    let cert = root
        .get("websocket")
        .and_then(|w| w.get("tls"))
        .and_then(|t| t.get("cert"))
        .and_then(|c| c.as_str())
        .unwrap()
        .to_string();
    assert_eq!(Path::new(&cert), std::path::absolute("cert.pem").unwrap());

    assert!(text.contains("listen: 0.0.0.0:7837"), "{text}");
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
            burrow::wire::secret::Secret::generate().encode()
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
) -> Result<burrow::init::ConfigOutput, burrow::base::failure::Failure> {
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
            secret: Some(burrow::wire::secret::Secret::generate().encode()),
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

#[test]
fn client_config_allows_mapped_ipv6_targets_as_ipv4() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    std::fs::write(
        &path,
        format!(
            "listen: 0.0.0.0:7835\nclients:\n  home:\n    secret: {}\nservices:\n  web: {{client: home, bind: 7840, target: \"[::ffff:127.0.0.1]:3000\"}}\n",
            burrow::wire::secret::Secret::generate().encode()
        ),
    )
    .unwrap();
    let out = client_config(
        None,
        &path,
        Some("h.example.com"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap();
    assert!(out.text.contains("127.0.0.1/32:3000"), "{}", out.text);
    let client = dir.path().join("client.yaml");
    std::fs::write(&client, &out.text).unwrap();
    assert!(matches!(load(&client), Loaded::Client(..)));
}

#[test]
fn client_config_explains_a_missing_or_empty_server_config() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("server.yaml");
    let err = client_config(
        None,
        &missing,
        Some("h.example.com"),
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert_eq!(err.exit, 2);
    assert!(err.message.contains("does not exist"), "{}", err.message);
    assert!(err.hint.unwrap().contains("init server"));

    std::fs::write(
        &missing,
        "listen: 0.0.0.0:7835\nclients: {}\nservices: {}\n",
    )
    .unwrap();
    let err = client_config(
        None,
        &missing,
        Some("h.example.com"),
        Format::Yaml,
        &ProcessEnv,
    )
    .err()
    .unwrap();
    assert!(
        err.message.ends_with("defines no clients"),
        "{}",
        err.message
    );
}

#[test]
fn visitor_init_asks_for_what_is_missing_and_validates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("visitor.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    let leaked: &'static str = Box::leak(secret.into_boxed_str());
    let mut p = Answers(vec!["laptop", leaked], Vec::new());
    let opts = InitOptions {
        output: Some(path.clone()),
        server: Some("127.0.0.1:1".into()),
        ..Default::default()
    };
    let mut out = Vec::new();
    burrow::init::init_visitor(&opts, &mut p, &mut out).unwrap();
    assert_eq!(
        p.1,
        [
            "visitor name (as in the server config)",
            "secret (from the server config)"
        ]
    );
    let Loaded::Visitor(v, _) = load(&path) else {
        panic!("not a visitor config")
    };
    assert_eq!(
        (v.name.as_str(), v.server.as_str()),
        ("laptop", "127.0.0.1:1")
    );
    assert!(v.forwards.is_empty());
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("without forwards: cannot ask the server"),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "init visitor --update -o {}",
            burrow::base::failure::path_arg(&path)
        )),
        "{out}"
    );
    assert!(
        out.contains("visitor-config laptop -o visitor.yaml"),
        "{out}"
    );
    #[cfg(unix)]
    assert_eq!(mode(&path), 0o600);
    let err = burrow::init::init_visitor(&opts, &mut NoPrompt, &mut Vec::new()).unwrap_err();
    assert!(err.message.contains("already exists"), "{}", err.message);
}

fn visitors_server(dir: &Path, visitors: &str, services: &str) -> std::path::PathBuf {
    let path = dir.join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    let mut visitors = visitors.to_string();
    while let Some(i) = visitors.find("@SECRET@") {
        visitors.replace_range(i..i + "@SECRET@".len(), &s());
    }
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: {}\nvisitors:\n{visitors}services:\n{services}",
            s()
        ),
    )
    .unwrap();
    path
}

#[test]
fn visitor_config_lists_the_visitors_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: any\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n  pg2: { client: home, target: \"10.0.0.2:5432\", visitors: [laptop] }\n  ssh: { client: home, target: 22, visitors: [laptop] }\n  web: { client: home, bind: 7840, target: 3000 }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    insta::assert_snapshot!(out.text.lines().filter(|l| !l.starts_with("secret:")).collect::<Vec<_>>().join("\n"), @"
    server: tunnel.example.com:7835
    name: laptop
    forwards:
      db:
        service: db
        local: 5432
      pg2:
        service: pg2
        local: 5433
      ssh:
        service: ssh
        local: 10022
      home:
        client: home
        socks: true
        local: 1080
    ");
    assert_eq!(out.notes.len(), 1, "{:?}", out.notes);
    assert!(out.notes[0].starts_with("127.0.0.1:1080 is a SOCKS5 proxy"));
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards.len(), 4);
}

#[test]
fn visitor_config_suggests_a_socks_target_inside_reach() {
    let dir = tempfile::tempdir().unwrap();
    for (reach, target) in [
        ("[192.168.150.0/25]", "192.168.150.1:22"),
        ("[\"10.0.0.7:8080-8090\"]", "10.0.0.7:8080"),
        ("[\"127.0.0.0/8:5432\"]", "127.0.0.1:5432"),
        ("any", "HOST:PORT"),
    ] {
        let path = visitors_server(
            dir.path(),
            &format!("  laptop:\n    secret: @SECRET@\n    reach:\n      home: {reach}\n"),
            "  web: { client: home, bind: 7840, target: 3000 }\n",
        );
        let out =
            burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
        assert!(
            out.notes[0].contains(&format!("--target {target} ")),
            "{reach}: {:?}",
            out.notes
        );
    }
}

#[test]
fn visitor_config_gives_socket_only_reach_a_forward_per_socket_instead_of_socks() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: [\"unix:/run/docker.sock\", \"unix:/run/apps/*\"]\n",
        "  web: { client: home, bind: 7840, target: 3000 }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    assert!(!out.text.contains("socks"), "{}", out.text);
    assert!(
        out.text.contains(
            "home-docker:\n    client: home\n    target: unix:/run/docker.sock\n    local: 10000"
        ),
        "{}",
        out.text
    );
    assert!(
        out.notes.iter().all(|n| !n.contains("SOCKS")),
        "{:?}",
        out.notes
    );
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards.len(), 1);
}

#[test]
fn visitor_config_explains_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n  idle:\n    secret: @SECRET@\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let make = |name: Option<&str>| {
        burrow::init::visitor_config(name, &path, None, Format::Yaml, &ProcessEnv)
    };
    let err = make(None).unwrap_err();
    assert_eq!(
        err.message,
        format!("{} defines 2 visitors; which one?", path.display())
    );
    assert!(
        err.hint.unwrap().contains("visitor-config idle -c"),
        "the example uses a real name"
    );
    let err = make(Some("idle")).unwrap_err();
    assert_eq!(
        err.message,
        format!(
            "visitor \"idle\" cannot reach anything in {}",
            path.display()
        )
    );
    let err = make(Some("lapto")).unwrap_err();
    assert_eq!(
        err.hint.unwrap(),
        "did you mean \"laptop\"? defined visitors: idle, laptop"
    );
    assert!(
        make(Some("laptop"))
            .unwrap()
            .text
            .contains("server: tunnel.example.com:7835")
    );
}

#[test]
fn visitor_config_writes_a_config_for_an_admin_only_visitor() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n  ops:\n    secret: @SECRET@\n    admin: write\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let out =
        burrow::init::visitor_config(Some("ops"), &path, None, Format::Yaml, &ProcessEnv).unwrap();
    assert!(out.text.contains("name: ops"), "{}", out.text);
    assert!(
        out.notes.iter().any(|n| n.contains("admin -c FILE")),
        "{:?}",
        out.notes
    );
}

#[test]
fn visitor_config_keeps_an_env_secret() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: ${BURROW_TEST_LAPTOP_SECRET}\n",
        "  db: { client: home, target: 5432, visitors: [laptop] }\n",
    );
    let env = |name: &str| {
        (name == "BURROW_TEST_LAPTOP_SECRET")
            .then(|| burrow::wire::secret::Secret::generate().encode())
    };
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &env).unwrap();
    assert!(
        out.text
            .contains("secret: \"${BURROW_TEST_LAPTOP_SECRET}\""),
        "{}",
        out.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn printed_visitor_config_reaches_the_private_service() {
    let dir = tempfile::tempdir().unwrap();
    let port = common::free_port();
    let echo = echo_server().await;
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n",
        &format!("  db: {{ client: home, target: \"{echo}\", visitors: [laptop] }}\n"),
    );
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("listen: 7835", &format!("listen: 127.0.0.1:{port}"));
    std::fs::write(&path, text).unwrap();
    let Loaded::Server(server, _) = load(&path) else {
        panic!()
    };
    let address = format!("127.0.0.1:{port}");
    let write = |name: &str, text: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, text).unwrap();
        load(&p)
    };
    let client = client_config(None, &path, Some(&address), Format::Yaml, &ProcessEnv).unwrap();
    let Loaded::Client(client, _) = write("client.yaml", &client.text) else {
        panic!()
    };
    let visitor =
        burrow::init::visitor_config(None, &path, Some(&address), Format::Yaml, &ProcessEnv)
            .unwrap();
    let Loaded::Visitor(mut visitor, _) = write("visitor.yaml", &visitor.text) else {
        panic!()
    };
    visitor.forwards.get_mut("db").unwrap().local = "127.0.0.1:0".parse().unwrap();

    let srv = start_server(*server).await;
    let mut cli = start_client(*client).await;
    wait_state(&mut cli.state, State::Connected).await;
    let shared = srv.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    let v = start_visitor(*visitor);
    assert_eq!(
        roundtrip(v.addr("db"), b"from a visitor").await,
        b"from a visitor"
    );
}

#[test]
fn visitor_config_gives_reach_a_socks_proxy_and_ranges_a_local_range() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n    reach:\n      home: any\n",
        "  game:\n    client: home\n    target: 30000-30010\n    visitors: [laptop]\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    for line in ["local: 30000-30010", "socks: true", "local: 1080"] {
        assert!(
            out.text.contains(line),
            "{line} missing from:\n{}",
            out.text
        );
    }
    assert_eq!(
        out.notes,
        [
            "127.0.0.1:1080 is a SOCKS5 proxy into client \"home\": point an app at it, or forward one target with: burrow visit --client home --target HOST:PORT --local 2222"
        ]
    );
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("a visitor config")
    };
    assert_eq!(v.forwards["game"].ports, 11);
    assert!(v.forwards["home"].socks);
}

#[test]
fn a_client_config_lists_ipv4_before_ipv6_and_unix_sockets_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\nclients:\n  home:\n    secret: {secret}\nservices:\n  a:\n    client: home\n    bind: 5433\n    target: unix:/run/a.sock\n  b:\n    client: home\n    bind: 5434\n    target: \"[2001:db8::5]:443\"\n  c:\n    client: home\n    bind: 5435\n    target: 10.0.0.9:22\n  d:\n    client: home\n    bind: 5436\n    target: \"[2001:db8::4]:8443\"\n  e:\n    client: home\n    bind: 5437\n    target: 10.0.0.8:80\n"
        ),
    )
    .unwrap();
    let text = client_config(
        Some("home"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    let allow: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(|l| l.trim_matches(['"', '\'']))
        .collect();
    assert_eq!(
        allow,
        [
            "10.0.0.8/32:80",
            "10.0.0.9/32:22",
            "[2001:db8::4/128]:8443",
            "[2001:db8::5/128]:443",
            "unix:/run/a.sock",
        ],
        "{text}"
    );
}

#[test]
fn client_config_allows_ranges_sockets_and_shared_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let secret = burrow::wire::secret::Secret::generate().encode();
    let other = burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\nclients:\n  home:\n    secret: {secret}\n  backup:\n    secret: {other}\nservices:\n  game:\n    client: home\n    bind: 30000-30010\n    target: 30000-30010\n  db:\n    client: home\n    bind: 5433\n    target: unix:/run/postgresql/.s.PGSQL.5432\n  web:\n    clients: [backup, home]\n    bind: 8080\n    target: 192.168.1.20:80\n"
        ),
    )
    .unwrap();
    let home = client_config(
        Some("home"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    for entry in [
        "127.0.0.1/32:30000-30010",
        "unix:/run/postgresql/.s.PGSQL.5432",
        "192.168.1.20/32:80",
    ] {
        assert!(home.contains(entry), "{entry} missing from:\n{home}");
    }
    let written = dir.path().join("client.yaml");
    std::fs::write(&written, &home).unwrap();
    assert!(matches!(load(&written), Loaded::Client(..)));
    let backup = client_config(
        Some("backup"),
        &path,
        Some("t.example"),
        Format::Yaml,
        &ProcessEnv,
    )
    .unwrap()
    .text;
    assert!(backup.contains("192.168.1.20/32:80"), "{backup}");
    assert!(!backup.contains("30000"), "{backup}");
}

#[test]
fn visitor_config_keeps_a_service_named_like_the_socks_forward() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: {}\nvisitors:\n  laptop:\n    secret: {}\n    reach:\n      home: any\nservices:\n  home:\n    client: home\n    target: 22\n    visitors: [laptop]\n  home-socks:\n    client: home\n    target: 80\n    visitors: [laptop]\n",
            s(),
            s()
        ),
    )
    .unwrap();
    let out = burrow::init::visitor_config(
        None,
        &path,
        None,
        burrow::config::Format::Yaml,
        &burrow::config::env::ProcessEnv,
    )
    .unwrap();
    assert!(
        out.text.contains("service: home-socks"),
        "the forward for service home-socks was overwritten:\n{}",
        out.text
    );
}

fn server(dir: &Path, client: &str, services: &str) -> std::path::PathBuf {
    let path = dir.join("server.yaml");
    let s = || burrow::wire::secret::Secret::generate().encode();
    std::fs::write(
        &path,
        format!(
            "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  {client}:\n    secret: {}\nvisitors:\n  laptop:\n    secret: {}\n    reach:\n      {client}: any\nservices:\n{services}",
            s(),
            s()
        ),
    )
    .unwrap();
    path
}

fn printed(dir: &Path, path: &Path) -> (String, burrow::config::Outcome) {
    let out = burrow::init::visitor_config(None, path, None, Format::Yaml, &ProcessEnv).unwrap();
    let written = dir.join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let loaded = load_file(&written, Some(Role::Visitor), &ProcessEnv).unwrap();
    (out.text, loaded)
}

#[test]
fn the_socks_forward_never_replaces_a_service_forward() {
    let dir = tempfile::tempdir().unwrap();
    let path = server(
        dir.path(),
        "home",
        "  home: { client: home, target: 5432, visitors: [laptop] }\n  home-socks: { client: home, target: 22, visitors: [laptop] }\n",
    );
    let (text, outcome) = printed(dir.path(), &path);
    let Some(Loaded::Visitor(v, _)) = outcome.loaded else {
        panic!("not a visitor config:\n{text}")
    };
    assert_eq!(v.forwards.len(), 3, "{text}");
    assert!(v.forwards.values().any(|f| f.socks), "{text}");
    assert!(
        v.forwards
            .values()
            .any(|f| f.to == burrow::wire::protocol::VisitTarget::service("home-socks")),
        "the home-socks service lost its forward:\n{text}"
    );
}

#[test]
fn printed_client_and_visitor_configs_stay_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let server = dir.path().join("server.yaml");
    std::fs::write(
        &server,
        "listen: 7835\npublic_address: tunnel.example.com\nclients:\n  home:\n    secret: [AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=, \"${HOME_SECRET}\"]\n  office:\n    secret: ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=\n  lab:\n    secret: QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8=\nvisitors:\n  laptop:\n    secret: YGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn8=\n    reach:\n      lab: [10.0.0.0/8, 192.168.1.0/24:22]\n  phone:\n    secret: \"${PHONE_SECRET}\"\nservices:\n  web: { client: home, bind: 7840, target: 3000 }\n  db:\n    clients: [home, office]\n    target: 10.0.0.5:5432\n    visitors: [laptop, phone]\n  app:\n    client: office\n    target: localhost:8080-8082\n    bind: 7850-7852\n  sock:\n    client: lab\n    target: unix:/run/app.sock\n    visitors: [laptop]\n  named:\n    client: lab\n    target: printer.lan:631\n    visitors: [phone]\n",
    )
    .unwrap();
    let env = |name: &str| match name {
        "HOME_SECRET" => Some("oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8=".to_string()),
        "PHONE_SECRET" => Some("gIGCg4SFhoeIiYqLjI2Oj5CRkpOUlZaXmJmam5ydnp8=".to_string()),
        _ => None,
    };
    let mut out = String::new();
    for format in [Format::Yaml, Format::Json] {
        for name in ["home", "office", "lab"] {
            for server_arg in [None, Some("ws://tunnel.example.com/burrow")] {
                let r = client_config(Some(name), &server, server_arg, format, &env);
                out.push_str(&format!("=== client {name} {format:?} {server_arg:?}\n"));
                match r {
                    Ok(o) => {
                        out.push_str(&format!("{}--- notes\n{}\n", o.text, o.notes.join("\n")))
                    }
                    Err(e) => out.push_str(&format!("error: {e:?}\n")),
                }
            }
        }
        for name in ["laptop", "phone"] {
            let r = burrow::init::visitor_config(Some(name), &server, None, format, &env);
            out.push_str(&format!("=== visitor {name} {format:?}\n"));
            match r {
                Ok(o) => out.push_str(&format!("{}--- notes\n{}\n", o.text, o.notes.join("\n"))),
                Err(e) => out.push_str(&format!("error: {e:?}\n")),
            }
        }
    }
    let out = out.replace(&dir.path().display().to_string(), "DIR");
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/printed_configs.txt");
    if std::env::var("BURROW_UPDATE_FIXTURES").as_deref() == Ok("1") {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &out).unwrap();
    }
    let want = std::fs::read_to_string(&golden)
        .expect("the golden file; record it once with BURROW_UPDATE_FIXTURES=1");
    assert_eq!(out, want, "client-config or visitor-config output changed");
}

#[test]
fn a_long_client_name_still_gives_a_valid_visitor_config() {
    let dir = tempfile::tempdir().unwrap();
    let long = "c".repeat(64);
    let path = server(
        dir.path(),
        &long,
        &format!("  {long}: {{ client: {long}, target: 5432, visitors: [laptop] }}\n"),
    );
    let (text, outcome) = printed(dir.path(), &path);
    assert!(
        !outcome.report.has_errors(),
        "{text}\n{}",
        outcome
            .report
            .render_text(burrow::base::style::Style { color: false })
    );
}

#[test]
fn visitor_config_writes_protocol_udp_for_udp_services() {
    let dir = tempfile::tempdir().unwrap();
    let path = visitors_server(
        dir.path(),
        "  laptop:\n    secret: @SECRET@\n",
        "  dns: { client: home, protocol: udp, target: 5353, visitors: [laptop] }\n  mdns: { client: home, target: 5353, visitors: [laptop] }\n  game: { client: home, protocol: udp, target: 27015-27016, visitors: [laptop] }\n",
    );
    let out = burrow::init::visitor_config(None, &path, None, Format::Yaml, &ProcessEnv).unwrap();
    insta::assert_snapshot!(out.text.lines().filter(|l| !l.starts_with("secret:")).collect::<Vec<_>>().join("\n"), @"
    server: tunnel.example.com:7835
    name: laptop
    forwards:
      dns:
        service: dns
        protocol: udp
        local: 5353
      game:
        service: game
        protocol: udp
        local: 27015-27016
      mdns:
        service: mdns
        local: 5353
    ");
    let written = dir.path().join("visitor.yaml");
    std::fs::write(&written, &out.text).unwrap();
    let Loaded::Visitor(v, _) = load(&written) else {
        panic!("not a visitor config")
    };
    assert_eq!(v.forwards["dns"].protocol, burrow_config::Protocol::Udp);
    assert_eq!(v.forwards["game"].protocol, burrow_config::Protocol::Udp);
    assert_eq!(v.forwards["mdns"].protocol, burrow_config::Protocol::Tcp);
}
