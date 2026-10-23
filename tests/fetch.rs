mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use burrow::allowlist::AllowList;
use burrow::client::State;
use burrow::secret::Secret;
use common::*;

struct World {
    server: RunningServer,
    settings: burrow::config::settings::ServerSettings,
    visitor: Secret,
    home: Secret,
    _client: Option<RunningClient>,
    dir: tempfile::TempDir,
}

async fn connect_home(server: &RunningServer, home: Secret) -> RunningClient {
    let mut c = client_settings(server.addr, home);
    c.allow = AllowList::Rules(vec![
        "127.0.0.0/8".parse().unwrap(),
        "192.168.50.0/23:22".parse().unwrap(),
    ]);
    let mut running = start_client(c).await;
    wait_state(&mut running.state, State::Connected).await;
    let shared = server.shared.clone();
    wait_for(|| shared.idle_channels("home") >= 1).await;
    running
}

impl World {
    async fn start(with_client: bool) -> World {
        logs();
        let home = Secret::generate();
        let visitor = Secret::generate();
        let mut settings = server_settings(vec![home.clone()]);
        settings
            .visitors
            .insert("laptop".into(), visitor_auth(visitor.clone(), &["home"]));
        let target = echo_server().await;
        settings
            .services
            .insert("db".into(), private_service(target, &["laptop"]));
        let server = start_server(settings.clone()).await;
        let client = if with_client {
            Some(connect_home(&server, home.clone()).await)
        } else {
            None
        };
        World {
            server,
            settings,
            visitor,
            home,
            _client: client,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn server_line(&self) -> String {
        if ws_mode() {
            format!("ws://{}/burrow", self.server.addr)
        } else {
            self.server.addr.to_string()
        }
    }

    fn burrow(&self, args: &[&str]) -> Output {
        self.burrow_with(args, &[])
    }

    fn burrow_with(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_burrow"))
            .args(args)
            .envs(vars.iter().copied())
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn new_config(&self, file: &str) -> Output {
        self.burrow(&[
            "init",
            "visitor",
            "--server",
            &self.server_line(),
            "--name",
            "laptop",
            "--secret",
            &self.visitor.encode(),
            "-o",
            file,
        ])
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.path().join(file)
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_visitor_config_gets_its_forwards_from_the_server() {
    let w = World::start(true).await;
    let out = w.new_config("visitor.yaml");
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.contains("with 2 forwards"), "{shown}");
    let yaml = read(&w.path("visitor.yaml"));
    assert!(yaml.contains("  db:\n    service: db\n"), "{yaml}");
    assert!(
        yaml.contains("  # client home allows 127.0.0.0/8, 192.168.50.0/23:22\n  home:\n    client: home\n    socks: true\n"),
        "{yaml}"
    );
    let check = w.burrow(&["validate", "-c", "visitor.yaml"]);
    assert!(check.status.success(), "{}", text(&check));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_adds_and_removes_forwards_and_keeps_the_rest_of_the_file() {
    let w = World::start(true).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let path = w.path("visitor.yaml");
    let edited = format!("# laptop at home\n{}# the end\n", read(&path));
    std::fs::write(&path, &edited).unwrap();

    let mut next = w.settings.clone();
    next.services.remove("db");
    let target = echo_server().await;
    next.services
        .insert("web".into(), private_service(target, &["laptop"]));
    w.server.reload(next).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let dry = w.burrow(&["init", "visitor", "--update"]);
    assert!(dry.status.success(), "{}", text(&dry));
    assert!(text(&dry).contains("- db"), "{}", text(&dry));
    assert!(text(&dry).contains("+ web"), "{}", text(&dry));
    assert!(text(&dry).contains("not written"), "{}", text(&dry));
    assert_eq!(read(&path), edited);

    let write = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(write.status.success(), "{}", text(&write));
    let yaml = read(&path);
    assert!(yaml.starts_with("# laptop at home\n"), "{yaml}");
    assert!(yaml.ends_with("# the end\n"), "{yaml}");
    assert!(yaml.contains("  web:\n    service: web\n"), "{yaml}");
    assert!(!yaml.contains("service: db"), "{yaml}");

    let again = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
    assert_eq!(read(&path), yaml);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_from_outside_a_services_allow_from_keeps_its_forward() {
    let w = World::start(true).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let path = w.path("visitor.yaml");
    let before = read(&path);
    let mut next = w.settings.clone();
    next.services.get_mut("db").unwrap().sources = burrow::allowlist::Sources {
        allow: vec!["10.0.0.0/8".parse().unwrap()],
        deny: vec![],
    };
    w.server.reload(next).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("- db"), "{}", text(&out));
    assert_eq!(read(&path), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_is_not_connected_is_marked_unknown() {
    let w = World::start(false).await;
    let out = w.new_config("visitor.yaml");
    assert!(out.status.success(), "{}", text(&out));
    let yaml = read(&w.path("visitor.yaml"));
    assert!(
        yaml.contains("# client home is offline; what it allows is unknown"),
        "{yaml}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_secret_is_explained_and_an_update_writes_nothing() {
    let w = World::start(true).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let path = w.path("visitor.yaml");
    let before = read(&path);
    let wrong = before.replace(&w.visitor.encode(), &Secret::generate().encode());
    std::fs::write(&path, &wrong).unwrap();
    let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(!out.status.success());
    assert!(
        text(&out).contains("rejected visitor \"laptop\""),
        "{}",
        text(&out)
    );
    assert_eq!(read(&path), wrong);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_json_visitor_config_is_written_and_updated() {
    let w = World::start(true).await;
    let out = w.new_config("visitor.json");
    assert!(out.status.success(), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_str(&read(&w.path("visitor.json"))).unwrap();
    assert_eq!(v["forwards"]["db"]["service"], "db");
    assert_eq!(v["forwards"]["home"]["socks"], true);
    let again = w.burrow(&["init", "visitor", "--update", "--yes", "-o", "visitor.json"]);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_is_only_for_visitors() {
    let w = World::start(false).await;
    let out = w.burrow(&["init", "client", "--update"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        text(&out).contains("--update is for visitor configs"),
        "{}",
        text(&out)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_secret_writes_no_new_file() {
    let w = World::start(true).await;
    let out = w.burrow(&[
        "init",
        "visitor",
        "--server",
        &w.server_line(),
        "--name",
        "laptop",
        "--secret",
        &Secret::generate().encode(),
        "-o",
        "visitor.yaml",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        text(&out).contains("rejected visitor \"laptop\""),
        "{}",
        text(&out)
    );
    assert!(!w.path("visitor.yaml").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unset_secret_variable_writes_the_file_without_asking() {
    let w = World::start(true).await;
    let out = w.burrow(&[
        "init",
        "visitor",
        "--server",
        &w.server_line(),
        "--name",
        "laptop",
        "--env-secret",
        "BURROW_FETCH_TEST_UNSET",
        "-o",
        "visitor.yaml",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    let shown = text(&out);
    assert!(shown.contains("BURROW_FETCH_TEST_UNSET"), "{shown}");
    assert!(shown.contains("so the server was not asked"), "{shown}");
    assert!(
        shown.contains("init visitor --update -o visitor.yaml"),
        "{shown}"
    );
    let yaml = read(&w.path("visitor.yaml"));
    assert!(yaml.contains("${BURROW_FETCH_TEST_UNSET}"), "{yaml}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_keeps_variables_comments_bom_and_crlf() {
    let w = World::start(true).await;
    let secret = w.visitor.encode();
    let file = format!(
        "\u{feff}# mine\r\nserver: {}\r\nname: laptop\r\nsecret: ${{LAPTOP_SECRET}}\r\nforwards:\r\n  # old one\r\n  gone:\r\n    service: gone\r\n    local: 9999\r\n  # keep me\r\n  db:\r\n    service: db\r\n    local: 15432\r\n# tail\r\n",
        w.server_line()
    );
    std::fs::write(w.path("visitor.yaml"), &file).unwrap();
    let vars = [("LAPTOP_SECRET", secret.as_str())];
    let out = w.burrow_with(&["init", "visitor", "--update", "--yes"], &vars);
    assert!(out.status.success(), "{}", text(&out));
    let yaml = read(&w.path("visitor.yaml"));
    assert!(yaml.starts_with("\u{feff}# mine\r\n"), "{yaml:?}");
    assert!(yaml.contains("secret: ${LAPTOP_SECRET}\r\n"), "{yaml:?}");
    assert!(
        yaml.contains("  # keep me\r\n  db:\r\n    service: db\r\n    local: 15432\r\n"),
        "{yaml:?}"
    );
    assert!(!yaml.contains("gone"), "{yaml:?}");
    assert!(!yaml.contains("old one"), "{yaml:?}");
    assert!(yaml.contains("  home:\r\n"), "{yaml:?}");
    assert!(yaml.ends_with("# tail\r\n"), "{yaml:?}");
    assert!(!yaml.replace("\r\n", "").contains('\n'), "{yaml:?}");
    let again = w.burrow_with(&["init", "visitor", "--update", "--yes"], &vars);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_refreshes_what_a_client_allows_once_it_is_known() {
    let w = World::start(false).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let path = w.path("visitor.yaml");
    assert!(read(&path).contains("client home is offline"));
    let _client = connect_home(&w.server, w.home.clone()).await;
    let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("~ home"), "{}", text(&out));
    assert!(
        text(&out).contains("(what it can reach changed)"),
        "{}",
        text(&out)
    );
    let yaml = read(&path);
    assert!(
        yaml.contains("  # client home allows 127.0.0.0/8, 192.168.50.0/23:22\n  home:\n"),
        "{yaml}"
    );
    assert!(!yaml.contains("offline"), "{yaml}");
    let again = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowed_reach_is_written_as_the_visitors_own_rules_and_hides_the_allow_list() {
    let w = World::start(true).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let mut next = w.settings.clone();
    next.visitors.get_mut("laptop").unwrap().reach = [(
        "home".to_string(),
        AllowList::Rules(vec![
            "127.0.0.0/8:22".parse().unwrap(),
            "unix:/run/app.sock".parse().unwrap(),
        ]),
    )]
    .into();
    w.server.reload(next).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("~ home"), "{}", text(&out));
    let yaml = read(&w.path("visitor.yaml"));
    assert!(
        yaml.contains(
            "  # client home lets you reach 127.0.0.0/8:22, unix:/run/app.sock\n  home:\n"
        ),
        "{yaml}"
    );
    assert!(
        !yaml.contains("192.168.50.0/23"),
        "the client's allow list leaked: {yaml}"
    );
    let again = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_note_that_shrank_to_a_prefix_of_the_old_one_is_still_updated() {
    let w = World::start(true).await;
    assert!(w.new_config("visitor.yaml").status.success());
    let rules = |r: &str| {
        let mut next = w.settings.clone();
        next.visitors.get_mut("laptop").unwrap().reach = [(
            "home".to_string(),
            AllowList::Rules(vec![r.parse().unwrap()]),
        )]
        .into();
        next
    };
    for r in ["127.0.0.0/8:2200-2210", "127.0.0.0/8:2200"] {
        w.server.reload(rules(r)).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
        assert!(out.status.success(), "{}", text(&out));
        let yaml = read(&w.path("visitor.yaml"));
        assert!(
            yaml.contains(&format!("  # client home lets you reach {r}\n  home:\n")),
            "{r}: {}\n{yaml}",
            text(&out)
        );
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_through_a_symlink_changes_the_real_file_and_keeps_the_link() {
    let w = World::start(true).await;
    std::fs::create_dir(w.path("real")).unwrap();
    assert!(w.new_config("real/visitor.yaml").status.success());
    std::os::unix::fs::symlink("real/visitor.yaml", w.path("visitor.yaml")).unwrap();
    let mut next = w.settings.clone();
    let target = echo_server().await;
    next.services
        .insert("web".into(), private_service(target, &["laptop"]));
    w.server.reload(next).await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let out = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(out.status.success(), "{}", text(&out));
    let link = std::fs::symlink_metadata(w.path("visitor.yaml")).unwrap();
    assert!(
        link.file_type().is_symlink(),
        "the link was replaced by a file"
    );
    assert!(read(&w.path("real/visitor.yaml")).contains("service: web"));
}

fn udp_world_settings(w: &World) -> burrow::config::settings::ServerSettings {
    let mut next = w.settings.clone();
    next.services.insert(
        "dns".into(),
        private_udp_service("127.0.0.1:53".parse().unwrap(), &["laptop"]),
    );
    next
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_visitor_writes_udp_forwards_and_update_adds_keeps_and_removes_them() {
    let w = World::start(true).await;
    let with_dns = udp_world_settings(&w);
    w.server.reload(with_dns.clone()).await;
    let shared = w.server.shared.clone();
    wait_for(|| shared.settings_snapshot().services.contains_key("dns")).await;
    let out = w.new_config("visitor.yaml");
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("udp 127.0.0.1:"), "{}", text(&out));
    let path = w.path("visitor.yaml");
    let yaml = read(&path);
    assert!(
        yaml.contains("  dns:\n    service: dns\n    protocol: udp\n    local: "),
        "{yaml}"
    );
    let check = w.burrow(&["validate", "-c", "visitor.yaml"]);
    assert!(check.status.success(), "{}", text(&check));

    let again = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(text(&again).contains("is up to date"), "{}", text(&again));
    assert_eq!(read(&path), yaml, "a granted UDP forward is kept as it is");

    let mut next = with_dns;
    next.services.remove("dns");
    next.services.insert(
        "voice".into(),
        private_udp_service("127.0.0.1:9987".parse().unwrap(), &["laptop"]),
    );
    w.server.reload(next).await;
    wait_for(|| shared.settings_snapshot().services.contains_key("voice")).await;
    let dry = w.burrow(&["init", "visitor", "--update"]);
    assert!(text(&dry).contains("- dns"), "{}", text(&dry));
    assert!(text(&dry).contains("+ voice"), "{}", text(&dry));
    let write = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(write.status.success(), "{}", text(&write));
    let yaml = read(&path);
    assert!(!yaml.contains("service: dns"), "{yaml}");
    assert!(
        yaml.contains("  voice:\n    service: voice\n    protocol: udp\n    local: 9987\n"),
        "{yaml}"
    );
}

async fn old(w: &World) -> std::net::SocketAddr {
    let grants = burrow::protocol::Grants {
        services: vec![burrow::protocol::GrantedService {
            name: "db".into(),
            protocol: burrow_config::Protocol::Tcp,
            port: 5432,
            ports: 1,
        }],
        clients: vec![],
    };
    old_server(w.visitor.clone(), grants.encode().unwrap()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_visitor_against_a_server_older_than_v012_lists_tcp_and_keeps_udp_forwards() {
    let w = World::start(false).await;
    let server = old(&w).await.to_string();
    let out = w.burrow(&[
        "init",
        "visitor",
        "--server",
        &server,
        "--name",
        "laptop",
        "--secret",
        &w.visitor.encode(),
        "-o",
        "visitor.yaml",
    ]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("older than v0.1.2"), "{}", text(&out));
    assert!(
        text(&out).contains("or lists more services than fit in one answer"),
        "{}",
        text(&out)
    );
    let path = w.path("visitor.yaml");
    let yaml = read(&path);
    assert!(yaml.contains("  db:\n    service: db\n"), "{yaml}");
    let mut edited = yaml.clone();
    edited.push_str("  dns:\n    service: dns\n    protocol: udp\n    local: 5353\n  gone:\n    service: gone\n    local: 7000\n");
    std::fs::write(&path, &edited).unwrap();
    let update = w.burrow(&["init", "visitor", "--update", "--yes"]);
    assert!(update.status.success(), "{}", text(&update));
    assert!(
        text(&update).contains("older than v0.1.2"),
        "{}",
        text(&update)
    );
    assert!(text(&update).contains("- gone"), "{}", text(&update));
    let yaml = read(&path);
    assert!(
        yaml.contains("  dns:\n    service: dns\n    protocol: udp\n    local: 5353\n"),
        "{yaml}"
    );
    assert!(!yaml.contains("gone"), "{yaml}");
}

fn visit_log(w: &World, config: &str) -> String {
    std::fs::write(w.path("visitor.yaml"), config).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            w.path("visitor.yaml"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_burrow"))
        .args(["visit", "-c", "visitor.yaml"])
        .current_dir(w.dir.path())
        .env("BURROW_INSTANCE_DIR", w.dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    let _ = child.kill();
    let out = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_udp_forward_to_a_server_older_than_v012_says_the_server_needs_updating() {
    let w = World::start(false).await;
    let server = old(&w).await;
    let log = visit_log(
        &w,
        &format!(
            "server: {server}\nname: laptop\nsecret: {}\nforwards:\n  dns: {{ service: dns, protocol: udp, local: {} }}\n",
            w.visitor.encode(),
            free_port()
        ),
    );
    assert!(
        log.contains("forward \"dns\": the server is older than v0.1.2"),
        "{log}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_forward_with_the_wrong_protocol_is_told_which_one_the_service_has() {
    let w = World::start(true).await;
    w.server.reload(udp_world_settings(&w)).await;
    let shared = w.server.shared.clone();
    wait_for(|| shared.settings_snapshot().services.contains_key("dns")).await;
    let log = visit_log(
        &w,
        &format!(
            "server: {}\nname: laptop\nsecret: {}\nforwards:\n  dns: {{ service: dns, local: {} }}\n  db: {{ service: db, protocol: udp, local: {} }}\n",
            w.server_line(),
            w.visitor.encode(),
            free_port(),
            free_port()
        ),
    );
    assert!(
        log.contains(
            "forward \"dns\": service \"dns\" is a udp service on the server; add \"protocol: udp\""
        ),
        "{log}"
    );
    assert!(
        log.contains("forward \"db\": service \"db\" is a tcp service on the server; remove \"protocol: udp\""),
        "{log}"
    );
}
