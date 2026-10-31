use burrow::config::Loaded;
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
    let o = burrow::config::load_file(
        &dir.path().join("server.yaml"),
        None,
        &burrow::config::env::ProcessEnv,
    )
    .unwrap();
    let strict: Vec<&str> = o
        .report
        .diagnostics
        .iter()
        .filter(|d| d.code != "low-port")
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        strict.is_empty(),
        "validate --strict passes, apart from this machine's low-port warning: {strict:?}"
    );
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

use burrow::base::prompt::Prompter;
use burrow::init::{WebInit, init_server_asking};

struct Script(Vec<&'static str>, Vec<String>);

impl Prompter for Script {
    fn ask(&mut self, q: &str) -> Option<String> {
        self.1.push(q.to_string());
        (!self.0.is_empty()).then(|| self.0.remove(0).to_string())
    }
    fn interactive(&self) -> bool {
        true
    }
    fn confirm(&mut self, q: &str, default: bool) -> bool {
        let a = self.ask(q);
        burrow::base::prompt::decide(a.as_deref(), default).unwrap_or(default)
    }
}

fn site(opts: InitOptions, prompt: &mut dyn Prompter) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.yaml");
    init_server_asking(
        &InitOptions {
            output: Some(path.clone()),
            ..opts
        },
        prompt,
        &mut Vec::new(),
    )
    .unwrap();
    load(&path);
    let text = std::fs::read_to_string(&path).unwrap();
    (dir, text)
}

const SITE_BLOCK: &str =
    "web:\n  https: 443\n  http: 80\n  letsencrypt:\n    email: ops@example.com\n";

#[test]
fn web_flags_write_the_web_block_and_the_site() {
    let (_d, text) = site(
        InitOptions {
            email: Some("ops@example.com".into()),
            web: Some(WebInit {
                domain: "blog.example.com".into(),
                client: None,
                target: Some("8080".into()),
            }),
            ..Default::default()
        },
        &mut burrow::base::prompt::NoPrompt,
    );
    assert!(text.contains(SITE_BLOCK), "{text}");
    assert!(
        text.contains("  blog:\n    client: home\n    target: 8080\n    web: blog.example.com\n"),
        "{text}"
    );
    assert!(
        !text.contains("bind:"),
        "the website replaces the example port: {text}"
    );
}

#[test]
fn the_website_question_is_asked_last_and_no_is_the_default() {
    let mut no = Script(vec![""], Vec::new());
    let (_d, text) = site(InitOptions::default(), &mut no);
    assert!(!text.contains("letsencrypt"), "{text}");
    assert_eq!(no.1.len(), 1);
    assert!(no.1[0].contains("Serve websites"), "{:?}", no.1);

    let mut yes = Script(
        vec!["y", "shop.example.com", "ops@example.com", "", "3000"],
        Vec::new(),
    );
    let (_d, text) = site(InitOptions::default(), &mut yes);
    assert!(text.contains(SITE_BLOCK), "{text}");
    assert!(text.contains("    web: shop.example.com\n"), "{text}");
    assert!(
        yes.1.iter().any(|q| q.contains("Let's Encrypt's terms")),
        "{:?}",
        yes.1
    );
    let mut no80 = Script(
        vec!["y", "shop.example.com", "ops@example.com", "n", "3000"],
        Vec::new(),
    );
    let (_d, text) = site(InitOptions::default(), &mut no80);
    assert!(!text.contains("http: 80"), "{text}");
}

#[test]
fn a_website_and_the_tunnel_share_port_443() {
    let (_d, text) = site(
        InitOptions {
            server: Some("tunnel.example.com".into()),
            websocket: Some(WebSocketInit::default()),
            email: Some("ops@example.com".into()),
            web: Some(WebInit {
                domain: "blog.example.com".into(),
                client: None,
                target: None,
            }),
            ..Default::default()
        },
        &mut burrow::base::prompt::NoPrompt,
    );
    assert!(text.contains(SITE_BLOCK), "{text}");
    assert!(
        text.contains("websocket:\n  listen: 443\n  host: tunnel.example.com\n"),
        "{text}"
    );
    assert!(text.contains("web: blog.example.com"), "{text}");
}

#[test]
fn the_printed_client_config_allows_the_websites_target_like_client_config_does() {
    let cases: [(&str, &[&str], bool); 5] = [
        ("8080", &["127.0.0.1/32:8080"], false),
        ("10.0.0.5:8080", &["10.0.0.5/32:8080"], false),
        ("[::1]:8080", &["[::1/128]:8080"], false),
        (
            "localhost:8080",
            &["127.0.0.1/32:8080", "[::1/128]:8080"],
            false,
        ),
        ("nas.lan:8080", &[], true),
    ];
    for (target, want, host_note) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        let mut out = Vec::new();
        init_server(
            &InitOptions {
                output: Some(path.clone()),
                server: Some("tun.example.com".into()),
                email: Some("ops@example.com".into()),
                web: Some(WebInit {
                    domain: "blog.example.com".into(),
                    client: None,
                    target: Some(target.into()),
                }),
                ..Default::default()
            },
            &mut out,
        )
        .unwrap();
        let printed = String::from_utf8(out).unwrap();
        let shown = printed
            .split("client config (save as client.yaml on the client machine):")
            .nth(1)
            .unwrap_or_else(|| panic!("{target}: {printed}"));
        let config = shown.split("\nnote: ").next().unwrap();
        let client_path = dir.path().join("client.yaml");
        std::fs::write(&client_path, config).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&client_path, mode).unwrap();
        }
        let Loaded::Client(client, _) = load(&client_path) else {
            panic!("{target}: {printed}");
        };
        let allow = client.allow.rules().unwrap_or_default();
        assert_eq!(allow, want, "{target}: {printed}");
        let note = "service \"blog\" targets host name nas.lan; add its IP address to \"allow\" on the client";
        assert_eq!(printed.contains(note), host_note, "{target}: {printed}");
        let listed = burrow::init::client_config(
            None,
            &path,
            Some("tun.example.com"),
            burrow::config::Format::Yaml,
            &burrow::config::env::ProcessEnv,
        )
        .unwrap();
        let allow_of = |text: &str| {
            text.split_once("allow:")
                .map(|(_, a)| a.trim_end().to_string())
        };
        assert_eq!(allow_of(config), allow_of(&listed.text), "{target}");
    }
}
