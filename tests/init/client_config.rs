use std::path::Path;

use burrow::base::prompt::NoPrompt;
use burrow::config::env::ProcessEnv;
use burrow::config::{Format, Loaded};
use burrow::init::{InitOptions, client_config, init_client, init_server};

use super::load;

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
