mod background;
mod logview;
pub(crate) mod ports;
pub(crate) mod system;
mod update;

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

use crate::config::model::Protocol;
use crate::config::{Loaded, Role, load_file};
use crate::diagnostics::Style;
use crate::failure::Failure;
use crate::instance::Identity;

pub use system::{Platform, binary_version, copy_file, is_admin};
pub use update::{update, update_from, worker as update_worker};

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

    pub fn command(&self) -> &str {
        match self.role.as_str() {
            "visitor" => "visit",
            role => role,
        }
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
    fn low_ports(&self, _m: &Manifest) -> bool {
        true
    }
    fn wildcard_low_ports(&self) -> bool {
        false
    }
    fn hidden(&self, _file: &Path) -> Option<String> {
        None
    }
    fn cannot_use(&self, m: &Manifest, file: &Path) -> Option<String> {
        if file.is_relative() {
            return Some("a service needs an absolute path".into());
        }
        (!self.can_read(&m.account, file))
            .then(|| format!("its account {} cannot read it", m.account))
    }
    fn secure_file(&self, path: &Path, account: &str) -> Result<(), Failure>;
    fn prepare_dirs(&self, account: &str) -> Result<(), Failure>;
    fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure>;
    fn read_env(&self, _m: &Manifest) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
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
    pub worker: Option<PathBuf>,
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
    fn values(n: &crate::config::tree::Node, out: &mut String) {
        use crate::config::tree::Value;
        match &n.value {
            Value::Str(s) => {
                out.push_str(s);
                out.push('\n');
            }
            Value::Seq(items) => items.iter().for_each(|i| values(i, out)),
            Value::Map(entries) => entries.iter().for_each(|e| values(&e.value, out)),
            _ => {}
        }
    }
    let mut text = String::new();
    match crate::config::tree::parse(src) {
        Ok(root) => values(&root, &mut text),
        Err(_) => text.push_str(src),
    }
    let mut out = Vec::new();
    let mut rest = text.as_str();
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

struct Uses {
    listens: Vec<(Protocol, SocketAddr, String)>,
    files: Vec<(String, PathBuf)>,
}

fn uses(loaded: &Loaded) -> Uses {
    let mut files = Vec::new();
    let mut listens = Vec::new();
    match loaded {
        Loaded::Server(s, cfg) => {
            if let Some(l) = s.listen {
                listens.push((Protocol::Tcp, l, "listen".into()));
            }
            if let Some(ws) = &s.websocket {
                listens.push((Protocol::Tcp, ws.listen, "websocket.listen".into()));
            }
            if let Some(m) = s.metrics {
                listens.push((Protocol::Tcp, m, "metrics.listen".into()));
            }
            for (name, svc) in &s.services {
                for (_, bind) in svc.binds() {
                    listens.push((svc.protocol, bind, format!("service \"{name}\"")));
                }
            }
            if let Some(tls) = cfg.websocket.as_ref().and_then(|w| w.tls.as_ref()) {
                files.push(("websocket.tls.cert".to_string(), PathBuf::from(&tls.cert)));
                files.push(("websocket.tls.key".to_string(), PathBuf::from(&tls.key)));
            }
        }
        Loaded::Client(_, cfg) => {
            if let Some(ca) = &cfg.ca {
                files.push(("ca".to_string(), PathBuf::from(ca)));
            }
        }
        Loaded::Visitor(s, cfg) => {
            if let Some(ca) = &cfg.ca {
                files.push(("ca".to_string(), PathBuf::from(ca)));
            }
            for (name, f) in &s.forwards {
                for (_, local) in f.listens() {
                    listens.push((Protocol::Tcp, local, format!("forward \"{name}\"")));
                }
            }
        }
    }
    Uses { listens, files }
}

pub fn misfits(name: &str, loaded: &Loaded) -> Vec<crate::diagnostics::Diagnostic> {
    match (system::backend(), load_manifest(name)) {
        (Ok(backend), Ok(m)) => misfits_of(&*backend, &m, loaded),
        _ => Vec::new(),
    }
}

fn needs_low_ports(backend: &dyn Backend, addr: &SocketAddr) -> bool {
    addr.port() < 1024 && !(backend.wildcard_low_ports() && addr.ip().is_unspecified())
}

fn misfits_of(
    backend: &dyn Backend,
    m: &Manifest,
    loaded: &Loaded,
) -> Vec<crate::diagnostics::Diagnostic> {
    let name = &m.name;
    let again = format!(
        "make the change in {} and install it again: {}",
        m.source_config.display(),
        crate::failure::admin(&install_args(m))
    );
    let Uses { listens, files } = uses(loaded);
    let mut out = Vec::new();
    let low: Vec<String> = listens
        .iter()
        .filter(|(_, a, _)| needs_low_ports(backend, a))
        .map(|(_, a, what)| format!("{what} on port {}", a.port()))
        .collect();
    if !low.is_empty() && !backend.low_ports(m) {
        out.push(
            crate::diagnostics::Diagnostic::error(
                "service-unit",
                format!(
                    "{name} was installed without permission for ports below 1024, so {} would fail when it restarts",
                    low.join(", ")
                ),
            )
            .help(again.clone()),
        );
    }
    for (field, file) in &files {
        if let Some(why) = backend.cannot_use(m, file) {
            out.push(
                crate::diagnostics::Diagnostic::error(
                    "service-unit",
                    format!("{name} cannot use {field} ({}): {why}", file.display()),
                )
                .help(again.clone()),
            );
        }
    }
    out
}

struct Checked {
    role: Role,
    identity: Option<Identity>,
    listens: Vec<(Protocol, SocketAddr, String)>,
    files: Vec<(String, PathBuf)>,
}

fn check_config(path: &Path, env: &BTreeMap<String, String>) -> Result<Checked, Failure> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let unset: Vec<String> = referenced_env(&text)
        .into_iter()
        .filter(|k| !env.contains_key(k))
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
            format!(
                "sudo drops environment variables; keep them with: sudo --preserve-env={list} {} (the values are saved for the service, readable by it only)",
                crate::failure::cmd(&format!("service install -c {}", crate::failure::path_arg(path)))
            )
        }));
    }
    let lookup = |k: &str| env.get(k).cloned();
    let outcome = load_file(path, None, &lookup).map_err(|e| {
        let (m, h) = e.describe(path);
        Failure::usage(m).hint(h)
    })?;
    if outcome.report.has_errors() {
        crate::say_err!("{}", outcome.report.render_text(Style::detect(false)));
        return Err(
            Failure::new(format!("{} has errors, not installing", path.display())).hint(format!(
                "fix them, then check with: {}",
                crate::failure::cmd(&format!("validate -c {}", crate::failure::path_arg(path)))
            )),
        );
    }
    let Some(loaded) = &outcome.loaded else {
        return Err(Failure::new(format!(
            "{} could not be loaded",
            path.display()
        )));
    };
    let (role, identity) = match loaded {
        Loaded::Server(..) => (Role::Server, Some(Identity::server(path))),
        Loaded::Client(s, _) => (
            Role::Client,
            Some(Identity::client(&s.endpoint.to_string(), &s.name)),
        ),
        Loaded::Visitor(s, _) => {
            if s.forwards.is_empty() {
                return Err(Failure::usage(format!(
                    "{} has no forwards, so a service would have nothing to run",
                    path.display()
                ))
                .hint("add one under \"forwards\", for example db: { service: db, local: 5432 }"));
            }
            (Role::Visitor, None)
        }
    };
    let Uses { listens, files } = uses(loaded);
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
    let src_text = std::fs::read_to_string(&source).unwrap_or_default();
    let env: BTreeMap<String, String> = match &opts.worker {
        Some(log) => background::take_secrets(log),
        None => referenced_env(&src_text)
            .into_iter()
            .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
            .collect(),
    };
    let checked = check_config(&source, &env)?;
    if let Ok(existing) = load_manifest(&name)
        && existing.source_config != source
    {
        return Err(Failure::usage(format!(
            "{name} is already installed from {}; its config is {}",
            existing.source_config.display(),
            existing.config.display()
        ))
        .hint(format!(
            "to add this config as another service, rename the file or pass --name, for example: --name {name}-2; to change {name} instead, edit {}",
            existing.config.display()
        )));
    }
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
    let home = platform.service_binary();
    let home_via_links = home
        .parent()
        .zip(home.file_name())
        .map(|(dir, file)| system::canonical_or_same(dir).join(file));
    let (binary, copy_binary) = if current == home || Some(&current) == home_via_links.as_ref() {
        (home.clone(), true)
    } else if system::trusted_binary(&current) {
        (current.clone(), false)
    } else {
        (home, true)
    };
    let low_ports = checked.listens.iter().any(|(_, a, _)| a.port() < 1024);
    let manifest = Manifest {
        name: name.clone(),
        role: checked.role.as_str().to_string(),
        config: config.clone(),
        source_config: source.clone(),
        env_file: env_file.clone(),
        log_file: platform.log_file(&short),
        binary: binary.clone(),
        copied_binary: copy_binary,
        account: account.clone(),
        created_account: false,
        low_ports,
        files: checked.files.iter().map(|(_, p)| p.clone()).collect(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let low: Vec<String> = checked
        .listens
        .iter()
        .filter(|(_, a, _)| needs_low_ports(&*backend, a))
        .map(|(_, a, what)| format!("{what} on {a}"))
        .collect();
    if !low.is_empty() && !backend.low_ports(&manifest) {
        return Err(Failure::new(format!(
            "{} would need root to listen below port 1024 on that address",
            low.join(", ")
        ))
        .hint("listen on all addresses (0.0.0.0) or on a port above 1023, or install with --run-as root"));
    }

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
    if was_running && opts.worker.is_none() {
        let mut args: Vec<std::ffi::OsString> = vec![
            "service".into(),
            "install".into(),
            "--config".into(),
            source.clone().into(),
            "--name".into(),
            name.clone().into(),
        ];
        if let Some(account) = &opts.run_as {
            args.push("--run-as".into());
            args.push(account.into());
        }
        if !opts.start {
            args.push("--no-start".into());
        }
        return background::Run::new("install")?.start(
            &format!("reinstalling {name}"),
            args,
            Some(&env),
            out,
        );
    }
    let _operation = system::operation()?;
    let mut again = StartAgain {
        backend: &*backend,
        manifest: previous.as_ref().filter(|_| was_running),
    };
    if let Some(p) = again.manifest {
        backend.stop(p)?;
    }

    if let Some(identity) = &checked.identity
        && let Err(e) = crate::instance::acquire(identity, &source)
    {
        return Err(Failure::new(format!("cannot install: {}", e.message)).hint(
            "stop the burrow process that is running outside the service first, then install again",
        ));
    }
    for (proto, addr, label) in &checked.listens {
        ports::check_free(*proto, *addr, label)?;
    }

    let created = backend.ensure_account(&account)?;
    let mut fresh = FreshAccount {
        backend: &*backend,
        account: created.then_some(account.as_str()),
    };
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
        if let Some(why) = backend.hidden(file) {
            return Err(Failure::new(format!(
                "the service cannot use {field} ({}): {why}",
                file.display()
            ))
            .hint("move the file, for example to /etc/burrow, and point the config at it"));
        }
        if !backend.can_read(&account, file) {
            return Err(Failure::new(format!(
                "the service account {account} cannot read {field} ({})",
                file.display()
            ))
            .hint(format!(
                "grant read access, for example: {}, or pass --run-as with an account that can read it",
                grant_hint(&account, file)
            )));
        }
    }

    backend.prepare_dirs(&account)?;
    let kept_binary = (again.manifest.is_some_and(|p| p.binary == binary)
        && copy_binary
        && binary.exists()
        && !system::same_content(&current, &binary))
    .then(|| binary.with_extension("reinstall"))
    .filter(|kept| system::copy_file(&binary, kept, true).is_ok());
    let _leftover = Leftover(kept_binary.clone());
    if copy_binary && !system::same_content(&current, &binary) {
        #[cfg(unix)]
        if binary != platform.binary_path()
            && let Some(dir) = binary.parent()
        {
            system::make_dir(dir, 0o755, None)?;
        }
        system::copy_file(&current, &binary, true)?;
        say(
            out,
            format!("copied {} -> {}", current.display(), binary.display()),
        )?;
        if let Some(usual) = platform.binary_path().parent()
            && binary != platform.binary_path()
        {
            say(
                out,
                format!(
                    "note: users other than root can change {}, so the service runs its own copy",
                    usual.display()
                ),
            )?;
        }
        let others: Vec<String> = all_manifests()
            .into_iter()
            .filter(|m| m.name != name && m.copied_binary && m.binary == binary)
            .map(|m| m.name)
            .collect();
        if !others.is_empty() {
            say(
                out,
                format!(
                    "note: {} still run the previous binary until restarted; restart them all with: {}",
                    others.join(", "),
                    crate::failure::admin("service update --force")
                ),
            )?;
        }
    }
    let undo = again.manifest.map(|p| Undo {
        manifest: p.clone(),
        config: std::fs::read(&p.config).ok(),
        env: backend.read_env(p),
        binary: kept_binary.clone().map(|kept| (kept, binary.clone())),
    });
    if source != config {
        system::copy_file(&source, &config, false)?;
        say(
            out,
            format!("copied {} -> {}", source.display(), config.display()),
        )?;
    }
    backend.secure_file(&config, &account)?;
    if let Some(f) = &env_file {
        backend.write_env(&manifest, &env)?;
        say(
            out,
            format!(
                "saved {} for the service in {} (readable by the service only)",
                env.keys().cloned().collect::<Vec<_>>().join(", "),
                f.display()
            ),
        )?;
    }
    backend.install(&manifest)?;
    if !opts.start {
        again.manifest = None;
    }
    save_manifest(&manifest)?;
    fresh.account = None;
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
        let started = backend.start(&manifest);
        again.manifest = None;
        if started.is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        let st = backend.status(&manifest);
        if let Some(undo) = undo.filter(|_| started.is_err() || !st.running) {
            let why = match started {
                Err(f) => f.message,
                Ok(()) => st.detail.clone(),
            };
            return Err(undo.put_back(&*backend, &manifest, &why, save_manifest, out));
        }
        started?;
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
                format!(
                    "  see why with: {}",
                    crate::failure::admin(&format!("service logs{arg}"))
                ),
            )?;
        }
    }
    if let Some(p) = &previous {
        release_previous(&*backend, p, &manifest, out)?;
    }
    say(
        out,
        format!(
            "\nthe service uses {}; change it with {} and it applies the change by itself",
            config.display(),
            crate::failure::admin(&format!("edit{arg}"))
        ),
    )?;
    say(
        out,
        format!(
            "  status: {}    logs: {}",
            crate::failure::cmd(&format!("service status{arg}")),
            crate::failure::admin(&format!("service logs{arg} -f"))
        ),
    )?;
    Ok(())
}

struct StartAgain<'a> {
    backend: &'a dyn Backend,
    manifest: Option<&'a Manifest>,
}

impl Drop for StartAgain<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.manifest.take() {
            let _ = self.backend.start(m);
        }
    }
}

struct Undo {
    manifest: Manifest,
    config: Option<Vec<u8>>,
    env: BTreeMap<String, String>,
    binary: Option<(PathBuf, PathBuf)>,
}

impl Undo {
    fn put_back(
        self,
        backend: &dyn Backend,
        new: &Manifest,
        why: &str,
        save: impl Fn(&Manifest) -> Result<(), Failure>,
        out: &mut dyn Write,
    ) -> Failure {
        let _ = backend.stop(new);
        let p = &self.manifest;
        let mut steps: Vec<Result<(), Failure>> = Vec::new();
        if let Some(bytes) = &self.config {
            steps.push(system::write_file_mode(&p.config, bytes, 0o600));
            steps.push(backend.secure_file(&p.config, &p.account));
        }
        if p.env_file.is_some() {
            steps.push(backend.write_env(p, &self.env));
        }
        if let Some((kept, binary)) = &self.binary {
            steps.push(system::copy_file(kept, binary, true));
            let _ = std::fs::remove_file(kept);
        }
        steps.push(backend.install(p));
        steps.push(save(p));
        steps.push(backend.start(p));
        if let Some(env) = &new.env_file
            && p.env_file.as_ref() != Some(env)
        {
            let _ = std::fs::remove_file(env);
        }
        if new.created_account && new.account != p.account {
            let _ = backend.remove_account(&new.account);
        }
        let mut failed = steps.into_iter().find_map(Result::err);
        if failed.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let st = backend.status(p);
            if !st.running {
                failed = Some(Failure::new(format!(
                    "the previous one does not run either: {}",
                    st.detail
                )));
            }
        }
        let _ = say(
            out,
            format!("error: the reinstalled {} did not run: {why}", new.name),
        );
        match failed {
            None => Failure::new(format!(
                "the reinstalled {} did not run, so its previous setup was put back and it runs again",
                new.name
            ))
            .hint(format!(
                "see why the new one failed with: {}",
                crate::failure::admin(&format!("service logs {} -n 50", new.name))
            )),
            Some(f) => Failure::new(format!(
                "the reinstalled {} did not run, and putting back its previous setup failed too: {}",
                new.name, f.message
            ))
            .hint(format!(
                "install it again from a config that works: {}",
                crate::failure::admin(&install_args(p))
            )),
        }
    }
}

fn release_previous(
    backend: &dyn Backend,
    old: &Manifest,
    new: &Manifest,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let others: Vec<Manifest> = all_manifests()
        .into_iter()
        .filter(|m| m.name != new.name)
        .collect();
    if let Some(env) = &old.env_file
        && new.env_file.as_ref() != Some(env)
        && std::fs::remove_file(env).is_ok()
    {
        say(
            out,
            format!(
                "removed {}, which the service no longer uses",
                env.display()
            ),
        )?;
    }
    if old.copied_binary
        && (old.binary != new.binary || !new.copied_binary)
        && !others
            .iter()
            .any(|o| o.copied_binary && o.binary == old.binary)
        && std::fs::remove_file(&old.binary).is_ok()
    {
        let _ = std::fs::remove_file(old.binary.with_extension("previous"));
        say(
            out,
            format!(
                "removed {}, which the service no longer uses",
                old.binary.display()
            ),
        )?;
    }
    if old.created_account && old.account != new.account {
        match others.iter().find(|o| o.account == old.account) {
            Some(heir) if !heir.created_account => {
                save_manifest(&Manifest {
                    created_account: true,
                    ..heir.clone()
                })?;
            }
            Some(_) => {}
            None => match backend.remove_account(&old.account) {
                Ok(()) => say(
                    out,
                    format!(
                        "removed service account {}, which nothing uses any more",
                        old.account
                    ),
                )?,
                Err(f) => say(
                    out,
                    format!(
                        "note: could not remove the old account {}: {}",
                        old.account, f.message
                    ),
                )?,
            },
        }
    }
    Ok(())
}

fn grant_hint(account: &str, file: &Path) -> String {
    let path = crate::failure::path_arg(file);
    if cfg!(windows) {
        format!("icacls {path} /grant \"{account}:R\"")
    } else if cfg!(target_os = "macos") {
        format!(
            "sudo chmod +a \"{account} allow read\" {path} (and \"allow search\" on its folders)"
        )
    } else {
        format!("sudo setfacl -m u:{account}:r {path} (and x on its folders)")
    }
}

struct Leftover(Option<PathBuf>);

impl Drop for Leftover {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct FreshAccount<'a> {
    backend: &'a dyn Backend,
    account: Option<&'a str>,
}

impl Drop for FreshAccount<'_> {
    fn drop(&mut self) {
        if let Some(account) = self.account.take() {
            let _ = self.backend.remove_account(account);
        }
    }
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
        && (m.log_file.as_os_str().is_empty() || m.log_file == p.log_file(m.short()))
        && (m.binary == p.service_binary()
            || ((cfg!(not(windows)) || !m.copied_binary) && system::trusted_binary(&m.binary)))
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
        return Err(
            Failure::new(format!("no burrow service named {name}")).hint(format!(
                "see the installed ones with: {}",
                crate::failure::cmd("service list")
            )),
        );
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
    system::write_file(&path, text.as_bytes())
}

pub fn any_installed() -> bool {
    std::fs::read_dir(Platform::current().state_dir()).is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.path().extension().is_some_and(|x| x == "json"))
    })
}

pub struct Installed {
    pub manifest: Manifest,
    pub running: bool,
}

pub fn installed() -> Vec<Installed> {
    let Ok(backend) = system::backend() else {
        return Vec::new();
    };
    all_manifests()
        .into_iter()
        .map(|manifest| Installed {
            running: backend.status(&manifest).running,
            manifest,
        })
        .collect()
}

pub fn env_of(m: &Manifest) -> BTreeMap<String, String> {
    system::backend().map(|b| b.read_env(m)).unwrap_or_default()
}

pub fn services_hidden() -> bool {
    let dir = Platform::current().state_dir();
    dir.exists() && std::fs::read_dir(&dir).is_err()
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
        [] => Err(Failure::new("no burrow service is installed").hint(format!(
            "install one with: {}",
            crate::failure::admin("service install")
        ))),
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

fn p_log_dir() -> PathBuf {
    Platform::current().log_dir()
}

pub fn uninstall(name: Option<&str>, purge: bool, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    resolve(name)?;
    let operation = system::operation()?;
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
        if let Some(dir) = m.log_file.parent().filter(|d| *d != p_log_dir()) {
            let _ = std::fs::remove_dir(dir);
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
    if m.copied_binary && !others.iter().any(|o| o.binary == m.binary) {
        match std::fs::remove_file(&m.binary) {
            Ok(()) => {
                let _ = std::fs::remove_file(m.binary.with_extension("previous"));
                let _ = std::fs::remove_file(m.binary.with_extension("old"));
                let _ = std::fs::remove_file(m.binary.with_extension("reinstall"));
                let _ = std::fs::remove_file(m.binary.with_extension("burrow-new"));
                let own_folder = cfg!(windows) || m.binary != Platform::current().binary_path();
                if own_folder && let Some(dir) = m.binary.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
                say(out, format!("removed {}", m.binary.display()))?;
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                let later = match m.binary.parent().filter(|_| cfg!(windows)) {
                    Some(dir) => format!("Remove-Item -Recurse {}", crate::failure::path_arg(dir)),
                    None => format!("sudo rm {}", crate::failure::path_arg(&m.binary)),
                };
                say(
                    out,
                    format!(
                        "note: cannot remove {} ({}); delete it after this command ends: {later}",
                        m.binary.display(),
                        crate::failure::file_reason(&e)
                    ),
                )?;
            }
            Err(_) => {}
        }
    }
    let heir = others
        .iter()
        .find(|o| o.account == m.account && !o.created_account);
    if m.created_account
        && let Some(heir) = heir
        && !others
            .iter()
            .any(|o| o.account == m.account && o.created_account)
    {
        let mut heir = heir.clone();
        heir.created_account = true;
        save_manifest(&heir)?;
    }
    let mut account_left = None;
    if m.created_account && !others.iter().any(|o| o.account == m.account) {
        #[cfg(unix)]
        {
            let dir = m
                .log_file
                .parent()
                .filter(|d| !kept.is_empty() && *d != p_log_dir());
            for f in kept.iter().map(PathBuf::as_path) {
                let _ = system::give_plain_file(f, 0, 0);
            }
            if let Some(d) = dir {
                let _ = std::os::unix::fs::lchown(d, Some(0), Some(0));
            }
        }
        match backend.remove_account(&m.account) {
            Ok(()) => say(out, format!("removed service account {}", m.account))?,
            Err(f) => account_left = Some(f),
        }
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
        operation.remove();
        let _ = std::fs::remove_dir_all(p.jobs_dir());
        let mut dirs = vec![p.state_dir()];
        dirs.extend(p.state_dir().parent().map(Path::to_path_buf));
        if purge {
            dirs.extend([p.log_dir(), p.config_dir()]);
        }
        let locks = crate::instance::service_dir().unwrap_or_else(crate::instance::default_dir);
        crate::instance::remove_unused(&locks);
        for dir in dirs {
            let _ = std::fs::remove_dir(dir);
        }
    }
    match account_left {
        Some(f) => Err(account_not_removed(&m.account, f)),
        None => Ok(()),
    }
}

fn account_not_removed(account: &str, f: Failure) -> Failure {
    let finish = match Platform::current() {
        Platform::MacOs => format!(
            "macOS lets only a local session or an app with full disk access delete accounts; finish in Terminal on the Mac with: sudo dscl . -delete /Users/{account}  (or allow full disk access for remote users under System Settings > General > Sharing > Remote Login, then run it over SSH)"
        ),
        _ => format!("finish with: sudo userdel {account}"),
    };
    Failure::new(format!(
        "everything else is removed, but the service account {account} is not: {}",
        f.message
    ))
    .hint(finish)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: (u64, u64, u64),
    pre: Vec<String>,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let ident = |a: &String, b: &String| match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => a.cmp(b),
        };
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self
                    .pre
                    .iter()
                    .zip(&other.pre)
                    .map(|(a, b)| ident(a, b))
                    .find(|o| o.is_ne())
                    .unwrap_or_else(|| self.pre.len().cmp(&other.pre.len())),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

pub fn parse_version(v: &str) -> Option<Version> {
    let v = v.trim().split([' ', '+']).next()?;
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, p.split('.').map(str::to_string).collect()),
        None => (v, Vec::new()),
    };
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let core = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some(Version { core, pre })
}

pub fn this_binary() -> Result<PathBuf, Failure> {
    let me = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate this burrow binary: {e}")))?;
    Ok(system::canonical(&me).unwrap_or(me))
}

pub fn as_worker(log: &Path, job: impl FnOnce(&mut dyn Write) -> Result<(), Failure>) -> u8 {
    background::worker(log, job)
}

fn interrupts(action: &str) -> bool {
    action == "restart" || (action == "reload" && cfg!(windows))
}

pub fn control(action: &str, name: Option<&str>, out: &mut dyn Write) -> Result<(), Failure> {
    system::backend()?;
    system::require_admin()?;
    let m = resolve(name)?;
    if !interrupts(action) {
        return control_now(action, Some(&m.name), out);
    }
    background::Run::new(action)?.start(
        &format!("restarting {}", m.name),
        vec!["service".into(), action.into(), m.name.clone().into()],
        None,
        out,
    )
}

pub fn control_now(action: &str, name: Option<&str>, out: &mut dyn Write) -> Result<(), Failure> {
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
        "reload" => backend.reload(&m)?,
        _ => unreachable!("known actions"),
    }
    let mut st = backend.status(&m);
    for _ in 0..15 {
        if st.running || action == "stop" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        st = backend.status(&m);
    }
    say(
        out,
        format!(
            "{}: {}",
            m.name,
            if st.running { "running" } else { "stopped" }
        ),
    )
}

fn install_args(m: &Manifest) -> String {
    let mut args = format!(
        "service install -c {}",
        crate::failure::path_arg(&m.source_config)
    );
    if service_name(&m.source_config, None).ok().as_deref() != Some(m.name.as_str()) {
        args.push_str(&format!(" --name {}", m.name));
    }
    args
}

pub fn status(name: Option<&str>, out: &mut dyn Write) -> Result<u8, Failure> {
    let backend = system::backend()?;
    let m = resolve(name)?;
    let st = backend.status(&m);
    let reinstall = crate::failure::admin(&install_args(&m));
    let state = match (st.installed, st.running) {
        (false, _) => format!("not installed in the service manager (run: {reinstall})"),
        (true, true) => "running".to_string(),
        (true, false) => "stopped".to_string(),
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
    if logview::open(&m.log_file).is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied) {
        say(out, "  recent:  (readable by administrators only)")?;
    }
    let recent = logview::last_lines(&m.log_file, 3);
    if !recent.is_empty() {
        say(out, "  recent:")?;
        for line in recent {
            say(out, format!("    {line}"))?;
        }
    }
    if m.source_config != m.config
        && m.source_config.exists()
        && differs(&m.source_config, &m.config)
    {
        say(
            out,
            format!(
                "  note:    {} differs from the installed copy; to use it, run: {}",
                m.source_config.display(),
                reinstall
            ),
        )?;
    }
    Ok(u8::from(!st.running))
}

fn differs(source: &Path, installed: &Path) -> bool {
    match (std::fs::read(source), std::fs::read(installed)) {
        (Ok(a), Ok(b)) => a != b,
        _ => false,
    }
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
    let mut retry = format!("service logs {}", m.name);
    if lines != 50 {
        retry.push_str(&format!(" -n {lines}"));
    }
    if follow {
        retry.push_str(" -f");
    }
    logview::show(
        &m.log_file,
        lines,
        follow,
        &crate::failure::admin(&retry),
        &mut out,
    )
}

pub fn run_service(name: &str) -> Result<u8, Failure> {
    system::run_as_service(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_installed_config_that_cannot_be_read_is_not_called_different() {
        let dir = tempfile::tempdir().unwrap();
        let (source, installed) = (dir.path().join("a.yaml"), dir.path().join("b.yaml"));
        std::fs::write(&source, "listen: 1\n").unwrap();
        assert!(!differs(&source, &dir.path().join("unreadable.yaml")));
        std::fs::write(&installed, "listen: 1\n").unwrap();
        assert!(!differs(&source, &installed));
        std::fs::write(&installed, "listen: 2\n").unwrap();
        assert!(differs(&source, &installed));
    }

    #[test]
    fn commands_that_interrupt_a_service_go_to_the_background() {
        assert!(interrupts("restart"));
        assert!(!interrupts("start"));
        assert!(!interrupts("stop"));
        assert_eq!(interrupts("reload"), cfg!(windows));
    }

    struct Recorder(std::cell::RefCell<Vec<String>>, bool);

    impl Recorder {
        fn note(&self, what: &str, m: &Manifest) -> Result<(), Failure> {
            self.0.borrow_mut().push(format!("{what} {}", m.account));
            Ok(())
        }
    }

    impl Backend for Recorder {
        fn kind(&self) -> &'static str {
            "recorder"
        }
        fn render(&self, _: &Manifest) -> String {
            String::new()
        }
        fn ensure_account(&self, _: &str) -> Result<bool, Failure> {
            Ok(false)
        }
        fn remove_account(&self, account: &str) -> Result<(), Failure> {
            self.0.borrow_mut().push(format!("remove {account}"));
            Ok(())
        }
        fn can_read(&self, _: &str, _: &Path) -> bool {
            true
        }
        fn secure_file(&self, _: &Path, _: &str) -> Result<(), Failure> {
            Ok(())
        }
        fn prepare_dirs(&self, _: &str) -> Result<(), Failure> {
            Ok(())
        }
        fn write_env(&self, m: &Manifest, env: &BTreeMap<String, String>) -> Result<(), Failure> {
            self.note(&format!("env {}", env.len()), m)
        }
        fn install(&self, m: &Manifest) -> Result<(), Failure> {
            self.note("install", m)
        }
        fn uninstall(&self, m: &Manifest) -> Result<(), Failure> {
            self.note("uninstall", m)
        }
        fn start(&self, m: &Manifest) -> Result<(), Failure> {
            self.note("start", m)
        }
        fn stop(&self, m: &Manifest) -> Result<(), Failure> {
            self.note("stop", m)
        }
        fn reload(&self, _: &Manifest) -> Result<(), Failure> {
            Ok(())
        }
        fn status(&self, _: &Manifest) -> Status {
            Status {
                installed: true,
                running: self.1,
                pid: None,
                detail: String::new(),
            }
        }
    }

    #[test]
    fn a_reinstall_that_does_not_run_puts_the_previous_one_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("home.yaml");
        std::fs::write(&config, "new config").unwrap();
        let old = Manifest {
            config: config.clone(),
            env_file: Some(dir.path().join("home.env")),
            account: "old".into(),
            ..record(Platform::Linux)
        };
        let new_env = dir.path().join("home-new.env");
        std::fs::write(&new_env, "SECRET=y").unwrap();
        let new = Manifest {
            config: config.clone(),
            env_file: Some(new_env.clone()),
            account: "new".into(),
            created_account: true,
            ..record(Platform::Linux)
        };
        let binary = dir.path().join("burrow");
        let kept = dir.path().join("burrow.previous");
        std::fs::write(&binary, "new binary").unwrap();
        std::fs::write(&kept, "old binary").unwrap();
        let undo = Undo {
            manifest: old.clone(),
            config: Some(b"old config".to_vec()),
            env: [("SECRET".to_string(), "x".to_string())].into(),
            binary: Some((kept.clone(), binary.clone())),
        };
        let rec = Recorder(Default::default(), true);
        let saved = std::cell::RefCell::new(Vec::new());
        let mut out = Vec::new();
        let f = undo.put_back(
            &rec,
            &new,
            "exited with status 3",
            |m| {
                saved.borrow_mut().push(m.account.clone());
                Ok(())
            },
            &mut out,
        );
        assert!(f.message.contains("runs again"), "{}", f.message);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "old config");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old binary");
        assert!(!kept.exists());
        assert!(!new_env.exists());
        assert_eq!(
            *rec.0.borrow(),
            [
                "stop new",
                "env 1 old",
                "install old",
                "start old",
                "remove new"
            ]
        );
        assert_eq!(*saved.borrow(), ["old"]);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("exited with status 3")
        );
    }

    #[test]
    fn low_ports_on_one_address_need_root_on_macos_only_away_from_the_wildcard() {
        let at = |a: &str| a.parse::<SocketAddr>().unwrap();
        let rec = Recorder(Default::default(), true);
        assert!(needs_low_ports(&rec, &at("0.0.0.0:80")));
        assert!(needs_low_ports(&rec, &at("127.0.0.1:80")));
        assert!(!needs_low_ports(&rec, &at("127.0.0.1:8080")));
        #[cfg(target_os = "macos")]
        {
            let mac = crate::service::launchd::Launchd;
            assert!(!needs_low_ports(&mac, &at("0.0.0.0:80")));
            assert!(needs_low_ports(&mac, &at("127.0.0.1:80")));
            let m = Manifest {
                account: "_burrow".into(),
                ..record(Platform::MacOs)
            };
            assert!(!mac.low_ports(&m));
        }
    }

    #[test]
    fn a_put_back_that_does_not_run_either_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let old = Manifest {
            config: dir.path().join("home.yaml"),
            env_file: None,
            ..record(Platform::Linux)
        };
        let undo = Undo {
            manifest: old.clone(),
            config: Some(b"old config".to_vec()),
            env: BTreeMap::new(),
            binary: None,
        };
        let f = undo.put_back(
            &Recorder(Default::default(), false),
            &old,
            "exited with status 3",
            |_| Ok(()),
            &mut Vec::new(),
        );
        assert!(f.message.contains("does not run either"), "{}", f.message);
    }

    fn record(p: Platform) -> Manifest {
        Manifest {
            name: "burrow-server".into(),
            role: "server".into(),
            config: p.config_dir().join("server.yaml"),
            source_config: PathBuf::from("server.yaml"),
            env_file: Some(p.config_dir().join("server.env")),
            log_file: p.log_file("server"),
            binary: p.service_binary(),
            copied_binary: true,
            account: p.default_account("burrow-server"),
            created_account: true,
            low_ports: false,
            files: Vec::new(),
            version: "0.1.0".into(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_edit_the_installed_unit_cannot_run_is_named_with_the_way_out() {
        let secret = crate::secret::Secret::generate().encode();
        let load = |text: String| {
            let nothing = |_: &str| None;
            let o = crate::config::load_str(
                "c.yaml",
                &text,
                crate::config::Format::Yaml,
                None,
                &nothing,
            );
            let shown = o
                .report
                .render_text(crate::diagnostics::Style { color: false });
            o.loaded.unwrap_or_else(|| panic!("{shown}"))
        };
        let server = |listen: &str| {
            load(format!(
                "listen: {listen}\nclients:\n  home: {{ secret: {secret} }}\nservices:\n  web: {{ client: home, bind: 8080, target: 80 }}\n"
            ))
        };
        let m = Manifest {
            name: "burrow-edge".into(),
            source_config: PathBuf::from("/srv/edge.yaml"),
            ..record(Platform::Linux)
        };
        let backend = systemd::Systemd;
        assert!(misfits_of(&backend, &m, &server("7835")).is_empty());
        let low = misfits_of(&backend, &m, &server("443"));
        assert_eq!(low.len(), 1);
        assert!(
            low[0].message.contains("listen on port 443"),
            "{}",
            low[0].message
        );
        assert!(
            low[0].help.as_deref().unwrap().contains("/srv/edge.yaml")
                && low[0]
                    .help
                    .as_deref()
                    .unwrap()
                    .contains("service install -c /srv/edge.yaml"),
            "{:?}",
            low[0].help
        );
        let allowed = Manifest {
            low_ports: true,
            ..m.clone()
        };
        assert!(misfits_of(&backend, &allowed, &server("443")).is_empty());
        let home = backend.cannot_use(&m, Path::new("/home/someone/ca.pem"));
        assert!(home.unwrap().contains("home folder"));
        assert!(
            backend
                .cannot_use(&m, Path::new("ca.pem"))
                .unwrap()
                .contains("absolute")
        );
        let private = backend.cannot_use(&m, Path::new("/tmp/ca.pem"));
        assert!(private.unwrap().contains("private"));
        assert!(
            backend
                .cannot_use(&m, Path::new("/var/tmp/x/ca.pem"))
                .is_some()
        );
        let as_root = Manifest {
            files: vec![PathBuf::from("/home/someone/old.pem")],
            account: "root".into(),
            ..m.clone()
        };
        assert_eq!(
            backend
                .cannot_use(&as_root, Path::new("/home/someone/ca.pem"))
                .map(|_| ()),
            Some(())
        );
        assert_eq!(
            backend.cannot_use(&as_root, Path::new("/etc/hostname")),
            None
        );
        assert!(
            backend
                .cannot_use(&as_root, Path::new("/etc/burrow-no-such.pem"))
                .unwrap()
                .contains("cannot read it")
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
        let v = |s: &str| parse_version(s).unwrap();
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.2.0") > v("1.2.0-rc.1"));
        assert!(v("1.2.0-rc.2") > v("1.2.0-rc.1"));
        assert!(v("1.2.0-rc.10") > v("1.2.0-rc.9"));
        assert!(v("1.2.0-rc.1") > v("1.2.0-rc"));
        assert!(v("1.2.0-beta") > v("1.2.0-alpha.9"));
        assert!(v("1.2.0-rc.1") > v("1.1.9"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert_eq!(v("1.2.3 (abc)"), v("1.2.3"));
        assert_eq!(parse_version("unknown version"), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }

    #[test]
    fn finds_referenced_environment_variables() {
        let src =
            "secret: ${HOME_SECRET}\nx: $${NOT_THIS}\nheaders: {a: ${TOKEN}, b: ${HOME_SECRET}}\n";
        assert_eq!(
            referenced_env(src),
            vec!["HOME_SECRET".to_string(), "TOKEN".to_string()]
        );
        let commented = "# old: ${GONE}\nsecret: ${HOME_SECRET} # was ${OLD}\n";
        assert_eq!(referenced_env(commented), vec!["HOME_SECRET".to_string()]);
        let json = "{\"secret\": \"${HOME_SECRET}\"}";
        assert_eq!(referenced_env(json), vec!["HOME_SECRET".to_string()]);
    }
}
