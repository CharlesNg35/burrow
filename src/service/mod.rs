mod logview;
pub(crate) mod ports;
mod system;

#[cfg(target_os = "macos")]
mod launchd;
#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows;

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::env::ProcessEnv;
use crate::config::model::Protocol;
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::failure::Failure;
use crate::instance::Identity;

pub use system::Platform;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub role: String,
    pub config: PathBuf,
    pub source_config: PathBuf,
    pub env_file: Option<PathBuf>,
    #[serde(default)]
    pub log_file: PathBuf,
    pub binary: PathBuf,
    pub copied_binary: bool,
    pub account: String,
    pub created_account: bool,
    pub low_ports: bool,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    pub version: String,
}

impl Manifest {
    pub fn short(&self) -> &str {
        self.name.strip_prefix("burrow-").unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub detail: String,
}

pub trait Backend {
    fn kind(&self) -> &'static str;
    fn render(&self, m: &Manifest) -> String;
    fn ensure_account(&self, account: &str) -> Result<bool, Failure>;
    fn remove_account(&self, account: &str) -> Result<(), Failure>;
    fn can_read(&self, account: &str, path: &Path) -> bool;
    fn secure_file(&self, path: &Path, account: &str) -> Result<(), Failure>;
    fn prepare_dirs(&self, account: &str) -> Result<(), Failure>;
    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure>;
    fn install(&self, m: &Manifest) -> Result<(), Failure>;
    fn uninstall(&self, m: &Manifest) -> Result<(), Failure>;
    fn start(&self, m: &Manifest) -> Result<(), Failure>;
    fn stop(&self, m: &Manifest) -> Result<(), Failure>;
    fn reload(&self, m: &Manifest) -> Result<(), Failure>;
    fn status(&self, m: &Manifest) -> Status;
}

pub struct InstallOptions {
    pub config: PathBuf,
    pub name: Option<String>,
    pub run_as: Option<String>,
    pub start: bool,
    pub print: bool,
}

fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    match writeln!(out, "{}", line.as_ref()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(Failure::new(format!("cannot write output: {e}")))
        }
        _ => Ok(()),
    }
}

pub fn service_name(config: &Path, given: Option<&str>) -> Result<String, Failure> {
    let raw = match given {
        Some(n) => n.to_string(),
        None => config
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "burrow".into()),
    };
    let clean: String = raw
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches('-').to_string();
    if clean.is_empty() || clean.len() > 48 {
        return Err(
            Failure::usage(format!("\"{raw}\" cannot be used as a service name"))
                .hint("pass --name with 1-48 letters, digits or dashes"),
        );
    }
    Ok(if clean == "burrow" || clean.starts_with("burrow-") {
        clean
    } else {
        format!("burrow-{clean}")
    })
}

pub fn referenced_env(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("${") {
        if i > 0 && rest.as_bytes()[i - 1] == b'$' {
            rest = &rest[i + 2..];
            continue;
        }
        let tail = &rest[i + 2..];
        if let Some(end) = tail.find('}') {
            let name = &tail[..end];
            if !name.is_empty() && !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
            rest = &tail[end + 1..];
        } else {
            break;
        }
    }
    out
}

struct Checked {
    role: Role,
    identity: Identity,
    listens: Vec<(Protocol, SocketAddr, String)>,
    files: Vec<(String, PathBuf)>,
}

fn check_config(path: &Path) -> Result<Checked, Failure> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let unset: Vec<String> = referenced_env(&text)
        .into_iter()
        .filter(|k| std::env::var_os(k).is_none())
        .collect();
    if !unset.is_empty() {
        let list = unset.join(",");
        return Err(Failure::usage(format!(
            "{} uses {} but {} not set here",
            path.display(),
            unset.iter().map(|k| format!("${{{k}}}")).collect::<Vec<_>>().join(", "),
            if unset.len() == 1 { "it is" } else { "they are" }
        ))
        .hint(if cfg!(windows) {
            format!("set it in this terminal first, for example: $env:{} = \"...\" (the values are saved for the service, readable by it only)", unset[0])
        } else {
            format!("sudo drops environment variables; keep them with: sudo --preserve-env={list} burrow service install (the values are saved for the service, readable by it only)")
        }));
    }
    let outcome = load_file(path, None, &ProcessEnv).map_err(|e| {
        let (m, h) = e.describe(path);
        Failure::usage(m).hint(h)
    })?;
    if outcome.report.has_errors() {
        eprint!("{}", outcome.report.render_text(Style::detect(false)));
        return Err(
            Failure::new(format!("{} has errors, not installing", path.display())).hint(format!(
                "fix them, then check with: burrow validate -c {}",
                path.display()
            )),
        );
    }
    let mut files = Vec::new();
    let mut listens = Vec::new();
    let (role, identity) = match outcome.loaded {
        Some(Loaded::Server(s, cfg)) => {
            if let Some(l) = s.listen {
                listens.push((Protocol::Tcp, l, "listen".into()));
            }
            if let Some(ws) = &s.websocket {
                listens.push((Protocol::Tcp, ws.listen, "websocket.listen".into()));
            }
            for (name, svc) in &s.services {
                listens.push((svc.protocol, svc.bind, format!("service \"{name}\"")));
            }
            if let Some(tls) = cfg.websocket.as_ref().and_then(|w| w.tls.as_ref()) {
                files.push(("websocket.tls.cert".to_string(), PathBuf::from(&tls.cert)));
                files.push(("websocket.tls.key".to_string(), PathBuf::from(&tls.key)));
            }
            (Role::Server, Identity::server(path))
        }
        Some(Loaded::Client(s, cfg)) => {
            if let Some(ca) = &cfg.ca {
                files.push(("ca".to_string(), PathBuf::from(ca)));
            }
            (
                Role::Client,
                Identity::client(&s.endpoint.to_string(), &s.name),
            )
        }
        None => {
            return Err(Failure::new(format!(
                "{} could not be loaded",
                path.display()
            )));
        }
    };
    for (field, file) in &files {
        if file.is_relative() {
            return Err(Failure::usage(format!(
                "{field} is a relative path ({}); a service needs absolute paths",
                file.display()
            ))
            .hint("the config is copied for the service, so write the full path, for example /etc/letsencrypt/live/host/fullchain.pem"));
        }
    }
    Ok(Checked {
        role,
        identity,
        listens,
        files,
    })
}

fn client_fields(path: &Path) -> Option<(String, String)> {
    let root = crate::config::tree::parse(&std::fs::read_to_string(path).ok()?).ok()?;
    let field = |k: &str| root.get(k).and_then(|n| n.as_str().map(str::to_string));
    let server = crate::transport::endpoint::Endpoint::parse(&field("server")?).ok()?;
    Some((server.to_string().to_ascii_lowercase(), field("name")?))
}

fn same_tunnel(existing: &Manifest, source: &Path, checked: &Checked) -> bool {
    if existing.role != checked.role.as_str() {
        return false;
    }
    if existing.source_config == source {
        return true;
    }
    checked.role == Role::Client
        && client_fields(source).is_some_and(|new| client_fields(&existing.config) == Some(new))
}

fn suggested_name(source: &Path, checked: &Checked) -> String {
    let base = match (checked.role, client_fields(source)) {
        (Role::Client, Some((server, name))) => {
            let host = crate::transport::endpoint::Endpoint::parse(&server)
                .ok()
                .map(|e| e.dial_address())
                .and_then(|a| a.rsplit_once(':').map(|(h, _)| h.to_string()))
                .unwrap_or_default();
            format!("{name}-{host}")
        }
        _ => source
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| format!("{}-{}", n.to_string_lossy(), checked.role.as_str()))
            .unwrap_or_else(|| format!("{}-2", checked.role.as_str())),
    };
    service_name(Path::new(&format!("{base}.yaml")), None)
        .unwrap_or_else(|_| format!("burrow-{}-2", checked.role.as_str()))
}

pub fn install(opts: &InstallOptions, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    let platform = Platform::current();
    let source = system::canonical(&opts.config).map_err(|e| {
        Failure::usage(format!(
            "cannot read {}: {}",
            opts.config.display(),
            crate::failure::file_reason(&e)
        ))
    })?;
    let name = service_name(&source, opts.name.as_deref())?;
    let checked = check_config(&source)?;
    if let Ok(existing) = load_manifest(&name)
        && !same_tunnel(&existing, &source, &checked)
    {
        return Err(Failure::usage(format!(
            "{name} is already installed for another {} config ({})",
            existing.role,
            existing.source_config.display()
        ))
        .hint(format!(
            "give this one its own name, for example: --name {}",
            suggested_name(&source, &checked)
        )));
    }
    let src_text = std::fs::read_to_string(&source).unwrap_or_default();
    let env: BTreeMap<String, String> = referenced_env(&src_text)
        .into_iter()
        .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
        .collect();
    let account = opts
        .run_as
        .clone()
        .unwrap_or_else(|| platform.default_account(&name));
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("yaml");
    let short = name.strip_prefix("burrow-").unwrap_or(&name).to_string();
    let config = platform.config_dir().join(format!("{short}.{ext}"));
    let env_file = (!env.is_empty()).then(|| platform.config_dir().join(format!("{short}.env")));
    let current = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate the burrow binary: {e}")))?;
    let current = system::canonical(&current).unwrap_or(current);
    let (binary, copy_binary) = match system::trusted_binary(&current) {
        true => (current.clone(), false),
        false => (platform.binary_path(), true),
    };
    let low_ports = checked.listens.iter().any(|(_, a, _)| a.port() < 1024);
    let manifest = Manifest {
        name: name.clone(),
        role: checked.role.as_str().to_string(),
        config: config.clone(),
        source_config: source.clone(),
        env_file: env_file.clone(),
        log_file: platform.log_dir().join(format!("{short}.log")),
        binary: binary.clone(),
        copied_binary: copy_binary,
        account: account.clone(),
        created_account: false,
        low_ports,
        files: checked.files.iter().map(|(_, p)| p.clone()).collect(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    if opts.print {
        say(
            out,
            format!(
                "# {} service {name}, as installed by: burrow service install",
                backend.kind()
            ),
        )?;
        say(out, backend.render(&manifest))?;
        return Ok(());
    }

    system::require_admin()?;
    let previous = load_manifest(&name).ok();
    let was_running = previous.as_ref().is_some_and(|p| backend.status(p).running);
    if let Some(p) = &previous
        && was_running
    {
        backend.stop(p)?;
    }
    let restore = |f: Failure| -> Failure {
        if let Some(p) = &previous
            && was_running
        {
            let _ = backend.start(p);
        }
        f
    };

    if let Err(e) = crate::instance::acquire(&checked.identity, &source) {
        return Err(restore(Failure::new(format!("cannot install: {}", e.message)).hint(
            "stop the burrow process that is running outside the service first, then install again",
        )));
    }
    for (proto, addr, label) in &checked.listens {
        if let Err(f) = ports::check_free(*proto, *addr, label) {
            return Err(restore(f));
        }
    }

    let created = backend.ensure_account(&account).map_err(restore)?;
    if created {
        say(
            out,
            format!("created service account {account} (no login, no home)"),
        )?;
    }
    let manifest = Manifest {
        created_account: created
            || previous
                .as_ref()
                .is_some_and(|p| p.created_account && p.account == account),
        ..manifest
    };
    for (field, file) in &checked.files {
        if !backend.can_read(&account, file) {
            return Err(restore(Failure::new(format!(
                "the service account {account} cannot read {field} ({})",
                file.display()
            ))
            .hint(format!(
                "grant read access, for example: sudo setfacl -m u:{account}:r {} (and x on its folders), or pass --run-as with an account that can read it",
                file.display()
            ))));
        }
    }

    backend.prepare_dirs(&account).map_err(restore)?;
    if copy_binary && !system::same_content(&current, &binary) {
        system::copy_file(&current, &binary, true).map_err(restore)?;
        say(
            out,
            format!("copied {} -> {}", current.display(), binary.display()),
        )?;
    }
    if source != config {
        system::copy_file(&source, &config, false).map_err(restore)?;
        say(
            out,
            format!("copied {} -> {}", source.display(), config.display()),
        )?;
    }
    backend.secure_file(&config, &account).map_err(restore)?;
    if let Some(f) = &env_file {
        backend.write_env(&manifest, &env).map_err(restore)?;
        say(
            out,
            format!(
                "saved {} for the service in {} (readable by the service only)",
                env.keys().cloned().collect::<Vec<_>>().join(", "),
                f.display()
            ),
        )?;
    }
    backend.install(&manifest).map_err(restore)?;
    save_manifest(&manifest)?;
    say(
        out,
        format!(
            "installed {} service {name} ({}, running as {account}, starts at boot, restarts on failure)",
            backend.kind(),
            manifest.role
        ),
    )?;
    let arg = if all_manifests().len() <= 1 {
        String::new()
    } else {
        format!(" {name}")
    };
    if opts.start {
        backend.start(&manifest)?;
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let st = backend.status(&manifest);
        if st.running {
            say(
                out,
                format!(
                    "started {name}{}",
                    st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
                ),
            )?;
        } else {
            say(
                out,
                format!("warning: {name} is not running: {}", st.detail),
            )?;
            say(
                out,
                format!("  see why with: sudo burrow service logs{arg}"),
            )?;
        }
    }
    let applies = if manifest.role == "server" {
        "changes reload automatically".to_string()
    } else {
        format!("then run: sudo burrow service restart{arg}")
    };
    say(
        out,
        format!(
            "\nthe service uses {}; edit that file, {applies}",
            config.display()
        ),
    )?;
    say(
        out,
        format!("  status: burrow service status{arg}    logs: sudo burrow service logs{arg} -f"),
    )?;
    Ok(())
}

pub fn manifest_path(name: &str) -> PathBuf {
    Platform::current().state_dir().join(format!("{name}.json"))
}

fn trustworthy(m: &Manifest, file: &Path) -> bool {
    let p = Platform::current();
    let in_dir = |path: &Path, dir: &Path| path.parent() == Some(dir);
    file.file_stem().is_some_and(|s| *s == *m.name)
        && (m.name == "burrow" || m.name.starts_with("burrow-"))
        && in_dir(&m.config, &p.config_dir())
        && m.env_file
            .as_deref()
            .is_none_or(|e| in_dir(e, &p.config_dir()))
        && (m.log_file.as_os_str().is_empty() || in_dir(&m.log_file, &p.log_dir()))
        && (m.binary == p.binary_path() || system::trusted_binary(&m.binary))
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("damaged: {e}"))?;
    if trustworthy(&m, path) {
        Ok(m)
    } else {
        Err("it points outside burrow's own folders, so it was not written by burrow".into())
    }
}

pub fn load_manifest(name: &str) -> Result<Manifest, Failure> {
    let path = manifest_path(name);
    if !path.exists() {
        return Err(Failure::new(format!("no burrow service named {name}"))
            .hint("see the installed ones with: burrow service list"));
    }
    read_manifest(&path).map_err(|why| {
        Failure::new(format!("ignoring {}: {why}", path.display())).hint(format!(
            "delete it as an administrator if you did not create it: {}",
            path.display()
        ))
    })
}

fn save_manifest(m: &Manifest) -> Result<(), Failure> {
    let path = manifest_path(&m.name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))?;
    }
    let text = serde_json::to_string_pretty(m).expect("manifest serializes");
    std::fs::write(&path, text)
        .map_err(|e| Failure::new(format!("cannot write {}: {e}", path.display())))
}

pub fn any_installed() -> bool {
    std::fs::read_dir(Platform::current().state_dir()).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
    })
}

pub fn all_manifests() -> Vec<Manifest> {
    let Ok(dir) = std::fs::read_dir(Platform::current().state_dir()) else {
        return Vec::new();
    };
    let mut list: Vec<Manifest> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_manifest(&e.path()).ok())
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

pub fn resolve(name: Option<&str>) -> Result<Manifest, Failure> {
    if let Some(n) = name {
        let full = if n.starts_with("burrow") {
            n.to_string()
        } else {
            format!("burrow-{n}")
        };
        return load_manifest(&full);
    }
    let all = all_manifests();
    match all.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(Failure::new("no burrow service is installed")
            .hint("install one with: sudo burrow service install")),
        many => Err(
            Failure::usage("more than one burrow service is installed; say which one").hint(
                format!(
                    "installed: {}",
                    many.iter()
                        .map(|m| m.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        ),
    }
}

pub fn uninstall(name: Option<&str>, purge: bool, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    let m = resolve(name)?;
    let _ = backend.stop(&m);
    backend.uninstall(&m)?;
    let _ = std::fs::remove_file(manifest_path(&m.name));
    say(out, format!("removed service {}", m.name))?;
    let others = all_manifests();
    let mut logs = vec![m.log_file.clone()];
    logs.extend(
        (1..=crate::logfile::KEEP).map(|n| PathBuf::from(format!("{}.{n}", m.log_file.display()))),
    );
    let mut kept = Vec::new();
    if purge {
        for f in [Some(&m.config), m.env_file.as_ref()].into_iter().flatten() {
            if std::fs::remove_file(f).is_ok() {
                say(out, format!("removed {}", f.display()))?;
            }
        }
        if logs
            .iter()
            .filter(|f| std::fs::remove_file(f).is_ok())
            .count()
            > 0
        {
            say(out, format!("removed the logs in {}", m.log_file.display()))?;
        }
    } else {
        kept.extend(
            [Some(&m.config), m.env_file.as_ref()]
                .into_iter()
                .flatten()
                .chain(logs.iter())
                .filter(|f| std::fs::symlink_metadata(f).is_ok_and(|m| m.file_type().is_file()))
                .cloned(),
        );
    }
    if m.copied_binary
        && !others.iter().any(|o| o.binary == m.binary)
        && std::fs::remove_file(&m.binary).is_ok()
    {
        let _ = std::fs::remove_file(m.binary.with_extension("previous"));
        let _ = std::fs::remove_file(m.binary.with_extension("old"));
        if cfg!(windows)
            && let Some(dir) = m.binary.parent()
        {
            let _ = std::fs::remove_dir(dir);
        }
        say(out, format!("removed {}", m.binary.display()))?;
    }
    if m.created_account && !others.iter().any(|o| o.account == m.account) {
        #[cfg(unix)]
        {
            let dir = m
                .log_file
                .parent()
                .filter(|_| others.is_empty() && !kept.is_empty());
            for f in kept.iter().map(PathBuf::as_path).chain(dir) {
                let _ = std::os::unix::fs::lchown(f, Some(0), Some(0));
            }
        }
        backend.remove_account(&m.account)?;
        say(out, format!("removed service account {}", m.account))?;
    }
    if !kept.is_empty() {
        let list = kept
            .iter()
            .map(|f| format!("\"{}\"", f.display()))
            .collect::<Vec<_>>();
        let delete = if cfg!(windows) {
            format!("Remove-Item {}", list.join(", "))
        } else {
            format!("sudo rm {}", list.join(" "))
        };
        say(
            out,
            format!("kept these files for a later reinstall; delete them with:\n  {delete}"),
        )?;
    }
    if others.is_empty() {
        let p = Platform::current();
        let mut dirs = vec![p.state_dir()];
        dirs.extend(p.state_dir().parent().map(Path::to_path_buf));
        if purge {
            dirs.extend([p.log_dir(), p.config_dir()]);
        }
        for dir in dirs {
            let _ = std::fs::remove_dir(dir);
        }
    }
    Ok(())
}

pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().split(['-', '+', ' ']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

pub fn update(name: Option<&str>, force: bool, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    let targets = match name {
        Some(_) => vec![resolve(name)?],
        None => all_manifests(),
    };
    if targets.is_empty() {
        return Err(Failure::new("no burrow service is installed")
            .hint("install one with: sudo burrow service install"));
    }
    let me = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate this burrow binary: {e}")))?;
    let me = system::canonical(&me).unwrap_or(me);
    let mine = env!("CARGO_PKG_VERSION");
    let mut binaries: Vec<PathBuf> = targets.iter().map(|m| m.binary.clone()).collect();
    binaries.sort();
    binaries.dedup();
    for binary in binaries {
        let users: Vec<&Manifest> = targets.iter().filter(|m| m.binary == binary).collect();
        let names = users
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if !users.iter().all(|m| m.copied_binary) {
            say(
                out,
                format!(
                    "{} is not managed by burrow; upgrade it where you installed it, then run: sudo burrow service restart",
                    binary.display()
                ),
            )?;
            continue;
        }
        if binary == me {
            say(out, format!("{names}: already uses this binary ({mine})"))?;
            continue;
        }
        let installed = system::binary_version(&binary);
        match (parse_version(mine), parse_version(&installed)) {
            (Some(a), Some(b)) if a == b && !force => {
                say(
                    out,
                    format!("{names}: already on {installed}, nothing to do"),
                )?;
                continue;
            }
            (Some(a), Some(b)) if a < b && !force => {
                say(
                    out,
                    format!(
                        "{names}: installed {installed} is newer than this binary ({mine}); not downgrading (use --force to do it anyway)"
                    ),
                )?;
                continue;
            }
            _ => {}
        }
        let running: Vec<&Manifest> = users
            .iter()
            .copied()
            .filter(|m| backend.status(m).running)
            .collect();
        for m in &running {
            backend.stop(m)?;
        }
        let previous = binary.with_extension("previous");
        let had_old = binary.exists() && system::copy_file(&binary, &previous, true).is_ok();
        if let Err(f) = system::copy_file(&me, &binary, true) {
            for m in &running {
                let _ = backend.start(m);
            }
            return Err(f);
        }
        let mut failed = Vec::new();
        for m in &running {
            if backend.start(m).is_err() {
                failed.push(m.name.clone());
                continue;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2000));
        for m in &running {
            if !backend.status(m).running && !failed.contains(&m.name) {
                failed.push(m.name.clone());
            }
        }
        if !failed.is_empty() {
            if had_old {
                let _ = system::copy_file(&previous, &binary, true);
                for m in &running {
                    let _ = backend.stop(m);
                    let _ = backend.start(m);
                }
            }
            return Err(Failure::new(format!(
                "{} did not start with {mine}; restored {installed}",
                failed.join(", ")
            ))
            .hint(format!("see why with: burrow service logs {}", failed[0])));
        }
        for m in &users {
            let mut updated = (*m).clone();
            updated.version = mine.to_string();
            save_manifest(&updated)?;
        }
        say(
            out,
            format!(
                "updated {} {installed} -> {mine}; restarted {names}",
                binary.display()
            ),
        )?;
        if had_old {
            say(
                out,
                format!("  the previous binary is kept at {}", previous.display()),
            )?;
        }
    }
    Ok(())
}

pub fn control(action: &str, name: Option<&str>, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    let m = resolve(name)?;
    match action {
        "start" => backend.start(&m)?,
        "stop" => backend.stop(&m)?,
        "restart" => {
            let _ = backend.stop(&m);
            backend.start(&m)?;
        }
        "reload" if m.role == "client" => {
            say(out, "clients apply config changes on restart; restarting")?;
            backend.stop(&m)?;
            backend.start(&m)?;
        }
        "reload" => backend.reload(&m)?,
        _ => unreachable!("known actions"),
    }
    let st = backend.status(&m);
    say(
        out,
        format!(
            "{}: {}",
            m.name,
            if st.running { "running" } else { "stopped" }
        ),
    )
}

pub fn status(name: Option<&str>, out: &mut dyn Write) -> Result<u8, Failure> {
    let backend = system::backend()?;
    let m = resolve(name)?;
    let st = backend.status(&m);
    let state = match (st.installed, st.running) {
        (false, _) => {
            "not installed in the service manager (run: sudo burrow service install again)"
        }
        (true, true) => "running",
        (true, false) => "stopped",
    };
    say(
        out,
        format!(
            "{}: {state}{}",
            m.name,
            st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
        ),
    )?;
    say(out, format!("  role:    {}", m.role))?;
    say(
        out,
        format!("  config:  {}{}", m.config.display(), missing(&m.config)),
    )?;
    say(
        out,
        format!(
            "  binary:  {} ({}){}",
            m.binary.display(),
            system::binary_version(&m.binary),
            missing(&m.binary)
        ),
    )?;
    say(out, format!("  account: {}", m.account))?;
    if !st.detail.is_empty() && !st.running {
        say(out, format!("  detail:  {}", st.detail))?;
    }
    say(out, format!("  logs:    {}", m.log_file.display()))?;
    if std::fs::File::open(&m.log_file)
        .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
    {
        say(out, "  recent:  (readable by administrators only)")?;
    }
    let recent = logview::last_lines(&m.log_file, 3);
    if !recent.is_empty() {
        say(out, "  recent:")?;
        for line in recent {
            say(out, format!("    {line}"))?;
        }
    }
    if m.source_config != m.config && m.source_config.exists() {
        let a = std::fs::read(&m.source_config).unwrap_or_default();
        let b = std::fs::read(&m.config).unwrap_or_default();
        if a != b {
            say(
                out,
                format!(
                    "  note:    {} differs from the installed copy; to use it, run: sudo burrow service install -c {}",
                    m.source_config.display(),
                    m.source_config.display()
                ),
            )?;
        }
    }
    Ok(u8::from(!st.running))
}

fn missing(p: &Path) -> &'static str {
    if p.exists() { "" } else { "  (missing!)" }
}

pub fn list(out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    let all = all_manifests();
    if all.is_empty() {
        return say(out, "no burrow services installed");
    }
    for m in all {
        let st = backend.status(&m);
        say(
            out,
            format!(
                "{:<24} {:<8} {:<8} {}",
                m.name,
                m.role,
                if st.running { "running" } else { "stopped" },
                m.config.display()
            ),
        )?;
    }
    Ok(())
}

pub fn logs(name: Option<&str>, lines: usize, follow: bool) -> Result<(), Failure> {
    let m = resolve(name)?;
    let mut out = std::io::stdout().lock();
    logview::show(&m.log_file, lines, follow, &mut out)
}

pub fn run_service(name: &str) -> Result<u8, Failure> {
    system::run_as_service(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(p: Platform) -> Manifest {
        Manifest {
            name: "burrow-server".into(),
            role: "server".into(),
            config: p.config_dir().join("server.yaml"),
            source_config: PathBuf::from("server.yaml"),
            env_file: Some(p.config_dir().join("server.env")),
            log_file: p.log_dir().join("server.log"),
            binary: p.binary_path(),
            copied_binary: true,
            account: p.default_account("burrow-server"),
            created_account: true,
            low_ports: false,
            files: Vec::new(),
            version: "0.1.0".into(),
        }
    }

    #[test]
    fn another_tunnel_with_the_same_name_is_not_a_reinstall() {
        let dir = tempfile::tempdir().unwrap();
        let secret = crate::secret::Secret::generate().encode();
        let write = |sub: &str, server: &str, name: &str| {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).unwrap();
            let f = d.join("client.yaml");
            std::fs::write(
                &f,
                format!("server: {server}\nname: {name}\nsecret: {secret}\nallow: [127.0.0.0/8]\n"),
            )
            .unwrap();
            f
        };
        let a = write("a", "vm-a.example.com", "home");
        let moved = write("moved", "VM-A.example.com:7835", "home");
        let b = write("b", "vm-b.example.com", "home");
        let mut existing = record(Platform::current());
        existing.role = "client".into();
        existing.source_config = a.clone();
        existing.config = a.clone();
        let checked = |p: &Path| check_config(p).unwrap();
        assert!(same_tunnel(&existing, &a, &checked(&a)));
        assert!(same_tunnel(&existing, &moved, &checked(&moved)));
        assert!(!same_tunnel(&existing, &b, &checked(&b)));
        assert_eq!(
            suggested_name(&b, &checked(&b)),
            "burrow-home-vm-b-example-com"
        );
    }

    #[test]
    fn only_records_inside_burrow_folders_are_trusted() {
        let p = Platform::current();
        let file = manifest_path("burrow-server");
        assert!(trustworthy(&record(p), &file));
        let outside = std::env::temp_dir().join("evil");
        let planted: [fn(&mut Manifest, &Path); 5] = [
            |m, o| m.binary = o.to_path_buf(),
            |m, o| m.config = o.to_path_buf(),
            |m, o| m.env_file = Some(o.to_path_buf()),
            |m, o| m.log_file = o.to_path_buf(),
            |m, _| m.name = "other".into(),
        ];
        for change in planted {
            let mut m = record(p);
            change(&mut m, &outside);
            assert!(!trustworthy(&m, &file), "{m:?}");
        }
        assert!(!trustworthy(&record(p), &manifest_path("burrow-client")));
    }

    #[test]
    fn names_come_from_the_config_file() {
        assert_eq!(
            service_name(Path::new("/x/server.yaml"), None).unwrap(),
            "burrow-server"
        );
        assert_eq!(
            service_name(Path::new("/x/Office Client.json"), None).unwrap(),
            "burrow-office-client"
        );
        assert_eq!(
            service_name(Path::new("/x/server.yaml"), Some("edge")).unwrap(),
            "burrow-edge"
        );
        assert_eq!(
            service_name(Path::new("/x/s.yaml"), Some("burrow-edge")).unwrap(),
            "burrow-edge"
        );
        assert!(service_name(Path::new("/x/s.yaml"), Some("!!!")).is_err());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_version("unknown version"), None);
    }

    #[test]
    fn finds_referenced_environment_variables() {
        let src =
            "secret: ${HOME_SECRET}\nx: $${NOT_THIS}\nheaders: {a: ${TOKEN}, b: ${HOME_SECRET}}\n";
        assert_eq!(
            referenced_env(src),
            vec!["HOME_SECRET".to_string(), "TOKEN".to_string()]
        );
    }
}
