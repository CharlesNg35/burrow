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
        text(&out).contains("(what it allows changed)"),
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
