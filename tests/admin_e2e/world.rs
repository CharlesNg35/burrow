use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_burrow");

pub use super::ports::{free_port, free_range};

pub fn secret() -> String {
    let out = Command::new(BIN)
        .arg("gensecret")
        .output()
        .expect("burrow gensecret");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub struct Proc {
    pub name: String,
    pub child: Child,
    pub log: PathBuf,
}

pub struct World {
    pub dir: tempfile::TempDir,
    pub server_bin: PathBuf,
    pub admin_bin: PathBuf,
    pub procs: Vec<Proc>,
    pub ports: BTreeMap<&'static str, u16>,
    pub secrets: BTreeMap<&'static str, String>,
}

#[derive(Default, Clone)]
pub struct Options {
    pub server_env: Vec<(String, String)>,
    pub clients: Option<Vec<&'static str>>,
    pub extra_services: String,
    pub extra_clients: String,
    pub extra_top: String,
    pub server_bin: Option<PathBuf>,
    pub admin_bin: Option<PathBuf>,
}

pub fn wait_until(what: &str, timeout: Duration, mut ok: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ok() {
        assert!(
            started.elapsed() < timeout,
            "timed out after {timeout:?} waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl World {
    pub fn new(opts: Options) -> World {
        let dir = tempfile::Builder::new()
            .prefix("w-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("a work dir");
        let mut w = World {
            dir,
            server_bin: opts.server_bin.clone().unwrap_or_else(|| BIN.into()),
            admin_bin: opts.admin_bin.clone().unwrap_or_else(|| BIN.into()),
            procs: Vec::new(),
            ports: BTreeMap::new(),
            secrets: BTreeMap::new(),
        };
        for k in ["listen", "web", "echo", "forward", "game", "https", "http"] {
            w.ports.insert(k, free_port());
        }
        for n in ["home", "office", "ops", "viewer", "laptop", "plain", "boss"] {
            w.secrets.insert(n, secret());
        }
        let p = &w.ports;
        let s = &w.secrets;
        let yaml = format!(
            "# server.yaml for the burrow admin end-to-end tests\n\
             # comments here must survive every change made through the admin UI\n\
             listen: 127.0.0.1:{listen}\npublic_address: 127.0.0.1:{listen}\n{extra_top}\n\
             clients:\n  # the home box\n  home:\n    secret: {home}\n  office:\n    secret: {office}\n    max_connections: 100\n{extra_clients}\n\
             visitors:\n  ops:\n    secret: {ops} # the admin\n    admin: write\n    admin_from: [127.0.0.1/32]\n  viewer:\n    secret: {viewer}\n    admin: read\n  laptop:\n    secret: {laptop}\n    reach:\n      home: [127.0.0.1/32]\n  plain:\n    secret: {plain}\n  boss:\n    secret: {boss}\n    admin: write\n\n\
             services:\n  web: {{ client: home, bind: 127.0.0.1:{web}, target: {echo} }} # keep this comment\n  ssh:\n    client: home\n    target: 22\n    visitors: [laptop]\n{extra}",
            listen = p["listen"],
            web = p["web"],
            echo = p["echo"],
            home = s["home"],
            office = s["office"],
            ops = s["ops"],
            viewer = s["viewer"],
            laptop = s["laptop"],
            plain = s["plain"],
            boss = s["boss"],
            extra = opts.extra_services,
            extra_clients = opts.extra_clients,
            extra_top = opts
                .extra_top
                .replace("HTTPS_PORT", &p["https"].to_string())
                .replace("HTTP_PORT", &p["http"].to_string()),
        );
        w.write("server.yaml", &yaml);
        let env: Vec<(&str, &str)> = opts
            .server_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let server = w.server_bin.clone();
        w.start_bin(&server, "server", &["server", "-c", "server.yaml"], &env);
        let started = Instant::now();
        while w.status().is_none() {
            if let Ok(Some(code)) = w.procs[0].child.try_wait() {
                panic!("the server exited ({code}):\n{}", w.logs());
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "the server did not start:\n{}",
                w.logs()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let clients = opts.clients.clone().unwrap_or_else(|| vec!["home"]);
        for c in &clients {
            w.start_client(c);
        }
        for c in &clients {
            w.wait_client(c, true);
        }
        w
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn write(&self, name: &str, text: &str) {
        let path = self.file(name);
        fs::write(&path, text).expect("write a test file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }

    pub fn start(&mut self, name: &str, args: &[&str], env: &[(&str, &str)]) -> &mut Proc {
        self.start_bin(Path::new(BIN), name, args, env)
    }

    pub fn start_bin(
        &mut self,
        bin: &Path,
        name: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> &mut Proc {
        let log = self.file(&format!("{name}.log"));
        let out = fs::File::create(&log).expect("a log file");
        let err = out.try_clone().expect("a log file");
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .current_dir(self.dir.path())
            .env("BURROW_INSTANCE_DIR", self.file(&format!("inst-{name}")))
            .env("BURROW_ADMIN_SESSION_DIR", self.file("admin-sessions"))
            .env("XDG_STATE_HOME", self.file("state"))
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("start burrow");
        self.procs.push(Proc {
            name: name.to_string(),
            child,
            log,
        });
        self.procs.last_mut().expect("just pushed")
    }

    pub fn stop(&mut self, name: &str) {
        for p in self.procs.iter_mut().filter(|p| p.name == name) {
            terminate(&mut p.child);
        }
    }

    pub fn status(&self) -> Option<serde_json::Value> {
        let out = Command::new(&self.server_bin)
            .args(["status", "--json", "-c"])
            .arg(self.file("server.yaml"))
            .env("BURROW_INSTANCE_DIR", self.file("inst-server"))
            .output()
            .ok()?;
        let all: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        all.get(0)?.get("status").cloned()
    }

    pub fn client_online(&self, name: &str) -> bool {
        self.status()
            .and_then(|s| s["clients"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .any(|c| c["name"] == name && c["connected"] == true)
    }

    pub fn wait_client(&self, name: &str, online: bool) {
        wait_until(
            &format!("client {name} online={online}"),
            Duration::from_secs(15),
            || self.client_online(name) == online,
        );
    }

    pub fn server_addr(&self) -> String {
        format!("127.0.0.1:{}", self.ports["listen"])
    }

    pub fn start_client(&mut self, name: &str) {
        self.start_client_with(name, "");
    }

    pub fn start_client_with(&mut self, name: &str, extra: &str) {
        let yaml = format!(
            "server: {}\nname: {name}\nsecret: {}\nallow: [127.0.0.1/32]\n{extra}",
            self.server_addr(),
            self.secrets[name]
        );
        let file = format!("{name}.yaml");
        self.write(&file, &yaml);
        self.start(name, &["client", "-c", &file], &[]);
    }

    pub fn visitor_file(&self, name: &str) -> String {
        let yaml = format!(
            "server: {}\nname: {name}\nsecret: {}\nforwards:\n  ssh: {{ service: ssh, local: {} }}\n",
            self.server_addr(),
            self.secrets[name],
            self.ports["forward"]
        );
        let file = format!("{name}.yaml");
        self.write(&file, &yaml);
        file
    }

    pub fn open_admin(&mut self, visitor: &str) -> String {
        self.open_admin_on(visitor, 0)
    }

    pub fn open_admin_on(&mut self, visitor: &str, port: u16) -> String {
        let port = port.to_string();
        let file = self.visitor_file(visitor);
        let url_file = self.file(&format!("url-{visitor}"));
        let url_arg = url_file.display().to_string();
        let name = format!("admin-{visitor}");
        let admin = self.admin_bin.clone();
        self.start_bin(
            &admin,
            &name,
            &[
                "admin",
                "-c",
                &file,
                "--no-open",
                "--port",
                &port,
                "--url-file",
                &url_arg,
                "--idle-exit",
                "5m",
            ],
            &[],
        );
        wait_until("the burrow admin url file", Duration::from_secs(20), || {
            fs::read_to_string(&url_file).is_ok_and(|s| !s.trim().is_empty())
        });
        fs::read_to_string(&url_file)
            .expect("url")
            .trim()
            .to_string()
    }

    pub fn logs(&self) -> String {
        let mut out = String::new();
        for p in &self.procs {
            out.push_str(&format!("===== {} =====\n", p.name));
            out.push_str(&fs::read_to_string(&p.log).unwrap_or_default());
        }
        out.push_str("===== server.yaml =====\n");
        out.push_str(&fs::read_to_string(self.file("server.yaml")).unwrap_or_default());
        out
    }

    pub fn config(&self) -> String {
        fs::read_to_string(self.file("server.yaml")).expect("server.yaml")
    }

    pub fn hand_edit(&self, f: impl FnOnce(&str) -> String) {
        let next = f(&self.config());
        let tmp = self.file("server.yaml.tmp");
        fs::write(&tmp, next).expect("write the hand edit");
        fs::rename(&tmp, self.file("server.yaml")).expect("replace server.yaml");
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

pub fn terminate(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for World {
    fn drop(&mut self) {
        for p in &mut self.procs {
            terminate(&mut p.child);
        }
    }
}
