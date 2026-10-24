mod background;
mod logview;
mod stray;
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

use crate::base::failure::Failure;
use crate::base::style::Style;
use crate::config::env::referenced_env;
use crate::config::{Loaded, Role, load_file};
use crate::install::{Manifest, Platform, all_manifests, load_manifest, manifest_path};
use crate::instance::Identity;
use burrow_config::Protocol;

pub use system::{binary_version, copy_file, is_admin};
pub use update::{update, update_from, worker as update_worker};

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
    fn made_for_burrow(&self, _account: &str) -> bool {
        false
    }
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
    fn config_folder(
        &self,
        dir: &Path,
        config: &Path,
        account: &str,
        writable: bool,
    ) -> Result<(), Failure> {
        #[cfg(unix)]
        {
            let owner = if writable { account } else { "root" };
            system::make_dir(dir, if writable { 0o700 } else { 0o755 }, Some(owner))?;
            if let Ok((uid, gid)) = crate::base::process::account_ids(owner) {
                give_history(config, uid, gid);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (account, config, writable);
            std::fs::create_dir_all(dir)
                .map_err(|e| Failure::new(format!("cannot create {}: {e}", dir.display())))
        }
    }
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
    fn journal_hint(&self, _m: &Manifest) -> Option<String> {
        None
    }
    fn units(&self) -> Vec<stray::Unit> {
        Vec::new()
    }
    fn manual_stop(&self, name: &str) -> String {
        format!("stop {name} in the service manager")
    }
}

pub struct InstallOptions {
    pub config: PathBuf,
    pub name: Option<String>,
    pub run_as: Option<String>,
    pub start: bool,
    pub print: bool,
    pub worker: Option<PathBuf>,
    pub admin_writable: bool,
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
    let name = if clean == "burrow" || clean.starts_with("burrow-") {
        clean
    } else {
        format!("burrow-{clean}")
    };
    let short = name.strip_prefix("burrow-").unwrap_or(&name);
    if OWN_FOLDERS.contains(&short) {
        let source = if given.is_some() {
            "--name"
        } else {
            "the config file name"
        };
        return Err(Failure::usage(format!(
            "\"{short}\" from {source} cannot be used as a service name: it is the name of one of burrow's own folders"
        ))
        .hint("pass --name with another name"));
    }
    Ok(name)
}

const OWN_FOLDERS: [&str; 5] = ["services", "jobs", "logs", "bin", "instances"];

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

pub fn misfits(name: &str, loaded: &Loaded) -> Vec<crate::config::diagnostics::Diagnostic> {
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
) -> Vec<crate::config::diagnostics::Diagnostic> {
    let name = &m.name;
    let again = format!(
        "make the change in {} and install it again: {}",
        m.source_config.display(),
        crate::base::failure::admin(&install_args(m, &m.source_config))
    );
    let Uses { listens, files } = uses(loaded);
    let admins = admin_visitors(loaded);
    let mut out = Vec::new();
    let low: Vec<String> = listens
        .iter()
        .filter(|(_, a, _)| needs_low_ports(backend, a))
        .map(|(_, a, what)| format!("{what} on port {}", a.port()))
        .collect();
    if !low.is_empty() && !backend.low_ports(m) {
        out.push(
            crate::config::diagnostics::Diagnostic::error(
                "service-unit",
                format!(
                    "{name} was installed without permission for ports below 1024, so {} would fail when it restarts",
                    low.join(", ")
                ),
            )
            .help(again.clone()),
        );
    }
    if !m.admin_writable
        && let Some(first) = admins.first()
    {
        out.push(
            crate::config::diagnostics::Diagnostic::warning(
                "service-admin-read-only",
                format!(
                    "visitor {first} has admin access, but {name} was installed without permission to change its config, so burrow admin is read-only"
                ),
            )
            .help(format!(
                "let the service change its config, without moving it: {}",
                crate::base::failure::admin(&writable_args(m))
            )),
        );
    }
    for (field, file) in &files {
        if let Some(why) = backend.cannot_use(m, file) {
            out.push(
                crate::config::diagnostics::Diagnostic::error(
                    "service-unit",
                    format!("{name} cannot use {field} ({}): {why}", file.display()),
                )
                .help(again.clone()),
            );
        }
    }
    out
}

fn admin_visitors(loaded: &Loaded) -> Vec<String> {
    match loaded {
        Loaded::Server(s, _) => s
            .visitors
            .iter()
            .filter(|(_, v)| v.admin.is_some())
            .map(|(n, _)| n.clone())
            .collect(),
        _ => Vec::new(),
    }
}

struct Checked {
    role: Role,
    identity: Option<Identity>,
    listens: Vec<(Protocol, SocketAddr, String)>,
    files: Vec<(String, PathBuf)>,
    admins: Vec<String>,
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
                crate::base::failure::cmd(&format!("service install -c {}", crate::base::failure::path_arg(path)))
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
                crate::base::failure::cmd(&format!(
                    "validate -c {}",
                    crate::base::failure::path_arg(path)
                ))
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
    let admins = admin_visitors(loaded);
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
        admins,
    })
}

fn recorded_source(existing: Option<&Manifest>, source: &Path) -> PathBuf {
    match existing {
        Some(m) if crate::base::fs::canonical_or_same(&m.config) == source => {
            m.source_config.clone()
        }
        _ => source.to_path_buf(),
    }
}

fn installs_from(existing: &Manifest, source: &Path) -> bool {
    existing.source_config == source
        || crate::base::fs::canonical_or_same(&existing.config) == source
}

pub fn install(opts: &InstallOptions, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    let platform = Platform::current();
    let source = crate::base::fs::canonical(&opts.config).map_err(|e| {
        Failure::usage(format!(
            "cannot read {}: {}",
            opts.config.display(),
            crate::base::failure::file_reason(&e)
        ))
    })?;
    let name = name_for(&source, opts.name.as_deref(), &all_manifests())?;
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
        && !installs_from(&existing, &source)
    {
        return Err(Failure::usage(format!(
            "{name} is already installed from {}; its config is {}",
            existing.source_config.display(),
            existing.config.display()
        ))
        .hint(format!(
            "to add this config as another service, rename the file or pass --name, for example: --name {name}-2; to change {name} instead, edit {} or install again from it: {}",
            existing.config.display(),
            crate::base::failure::admin(&format!(
                "service install -c {}",
                crate::base::failure::path_arg(&existing.config)
            ))
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
    let folder = platform.config_dir().join(&short);
    let writable =
        checked.role == Role::Server && (opts.admin_writable || !checked.admins.is_empty());
    let config = folder.join(format!("{}.{ext}", checked.role.as_str()));
    let env_file = (!env.is_empty()).then(|| platform.config_dir().join(format!("{short}.env")));
    let current = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate the burrow binary: {e}")))?;
    let current = crate::base::fs::canonical(&current).unwrap_or(current);
    let (binary, copy_binary) = service_binary_for(&current, platform);
    let low_ports = checked.listens.iter().any(|(_, a, _)| a.port() < 1024);
    let manifest = Manifest {
        name: name.clone(),
        role: checked.role.as_str().to_string(),
        config: config.clone(),
        source_config: recorded_source(load_manifest(&name).ok().as_ref(), &source),
        env_file: env_file.clone(),
        log_file: platform.log_file(&short),
        binary: binary.clone(),
        copied_binary: copy_binary,
        account: account.clone(),
        created_account: false,
        low_ports,
        files: checked.files.iter().map(|(_, p)| p.clone()).collect(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        admin_writable: writable,
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
    let taken = take_over(stray::named(&name).as_ref(), &config)?;
    let old_files: Vec<PathBuf> = taken
        .iter()
        .flat_map(Takeover::old_files)
        .chain(stray::leftovers(Some(&name)))
        .collect();
    let operation = system::operation(out)?;
    let previous = load_manifest(&name).ok();
    let was_running = previous.as_ref().is_some_and(|p| backend.status(p).running);
    if was_running && opts.worker.is_none() {
        drop(operation);
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
        if opts.admin_writable {
            args.push("--admin-writable".into());
        }
        return background::Run::new("install")?.start(
            &format!("reinstalling {name}"),
            args,
            Some(&env),
            out,
        );
    }
    let _operation = operation;
    let mut again = StartAgain {
        backend: &*backend,
        manifest: previous.as_ref().filter(|_| was_running),
    };
    if let Some(p) = again.manifest {
        backend.stop(p)?;
    }

    if let Some(e) = checked.identity.as_ref().and_then(crate::instance::held) {
        return Err(Failure::new(format!("cannot install: {}", e.message)).hint(
            "stop the burrow process that is running outside the service first, then install again",
        ));
    }
    let users = || {
        let managed = all_manifests().into_iter().filter_map(|m| {
            backend
                .status(&m)
                .pid
                .map(|pid| crate::base::ports::PortUser {
                    pid,
                    name: m.name,
                    managed: true,
                    stop: None,
                })
        });
        let strays = stray::strays().into_iter().filter_map(|s| {
            s.pid.map(|pid| crate::base::ports::PortUser {
                pid,
                name: s.label,
                managed: false,
                stop: Some(s.stop),
            })
        });
        managed.chain(strays).collect()
    };
    for (proto, addr, label) in &checked.listens {
        crate::base::ports::check_free(*proto, *addr, label, &users)?;
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
        created_account: burrows_account(
            created,
            &account,
            &platform.default_account(&name),
            previous.as_ref(),
            || backend.made_for_burrow(&account),
        ),
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
        place_binary(&current, &binary, &platform.binary_path())?;
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
                    crate::base::failure::admin("service update --force")
                ),
            )?;
        }
    }
    let undo = again.manifest.map(|p| Undo {
        manifest: p.clone(),
        config: system::read_plain_file(&p.config),
        env: backend.read_env(p),
        binary: kept_binary.clone().map(|kept| (kept, binary.clone())),
    });
    backend.config_folder(&folder, &config, &account, writable)?;
    if source != crate::base::fs::canonical_or_same(&config) {
        if writable {
            match keep_admin_copy(&source, &config)? {
                Kept::Version => say(
                    out,
                    "kept the installed config as a history version before replacing it; revert to it from burrow admin Activity if needed",
                )?,
                Kept::NotPlain => say(
                    out,
                    format!(
                        "note: {} is not a plain file, so it is replaced without keeping it",
                        config.display()
                    ),
                )?,
                Kept::Nothing => {}
            }
        }
        system::copy_file(&source, &config, false)?;
        say(
            out,
            format!("copied {} -> {}", source.display(), config.display()),
        )?;
    }
    backend.secure_file(&config, &account)?;
    backend.config_folder(&folder, &config, &account, writable)?;
    if writable {
        let why = match checked.admins.as_slice() {
            [] => "--admin-writable was given".to_string(),
            [one] => format!("visitor {one} has admin access"),
            many => format!("visitors {} have admin access", many.join(", ")),
        };
        say(
            out,
            format!(
                "the service may change its config in {} because {why} (burrow admin)",
                folder.display()
            ),
        )?;
    }
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
    let mut own = own_logs(&manifest.log_file);
    own.push(config.clone());
    own.extend(manifest.env_file.clone());
    let given = give_old_files(&old_files, &account, &own);
    if let Some(t) = &taken {
        say(out, format!("note: {}", t.note(&given)))?;
    }
    if let Some(note) = given_note(
        &name,
        &given,
        taken.as_ref().and_then(|t| t.old_config.as_ref()),
    ) {
        say(out, format!("note: {note}"))?;
    }
    let arg = name_arg(&name, all_manifests().len(), stray::strays().len());
    if opts.start {
        start_installed(
            &*backend,
            &manifest,
            &arg,
            &mut again,
            undo,
            save_manifest,
            out,
        )?;
    }
    if let Some(p) = &previous {
        release_previous(&*backend, p, &manifest, out)?;
    }
    say(
        out,
        format!(
            "\nthe service uses {}; change it with {} and it applies the change by itself",
            config.display(),
            crate::base::failure::admin(&format!("edit{arg}"))
        ),
    )?;
    say(
        out,
        format!(
            "  status: {}    logs: {}",
            crate::base::failure::cmd(&format!("service status{arg}")),
            crate::base::failure::admin(&format!("service logs{arg} -f"))
        ),
    )?;
    Ok(())
}

fn name_arg(name: &str, managed: usize, strays: usize) -> String {
    if managed + strays <= 1 {
        String::new()
    } else {
        format!(" {name}")
    }
}

fn burrows_account(
    created: bool,
    account: &str,
    default: &str,
    previous: Option<&Manifest>,
    made_for_burrow: impl FnOnce() -> bool,
) -> bool {
    created
        || previous.is_some_and(|p| p.created_account && p.account == account)
        || (account == default && made_for_burrow())
}

#[derive(Debug)]
struct Takeover {
    label: String,
    old_config: Option<PathBuf>,
}

impl Takeover {
    fn old_files(&self) -> Vec<PathBuf> {
        self.old_config.iter().cloned().collect()
    }

    fn note(&self, given: &[PathBuf]) -> String {
        let replaced = format!("replaced {}, which burrow had no record of", self.label);
        match &self.old_config {
            Some(old) if given.contains(old) => format!(
                "{replaced}; its old config was left as it was, now owned by root: {}",
                old.display()
            ),
            Some(old) => format!(
                "{replaced}; its old config was left as it was: {}",
                old.display()
            ),
            None => replaced,
        }
    }
}

fn take_over(stray: Option<&stray::Stray>, config: &Path) -> Result<Option<Takeover>, Failure> {
    let Some(s) = stray else {
        return Ok(None);
    };
    if s.running {
        return Err(s.failure());
    }
    Ok(Some(Takeover {
        label: s.label.clone(),
        old_config: s.config.clone().filter(|c| c != config),
    }))
}

fn given_note(name: &str, given: &[PathBuf], shown: Option<&PathBuf>) -> Option<String> {
    let rest: Vec<String> = given
        .iter()
        .filter(|p| Some(*p) != shown)
        .map(|p| p.display().to_string())
        .collect();
    (!rest.is_empty()).then(|| {
        format!(
            "gave {}, left by an older install of {name}, to root, since the service does not use {}",
            rest.join(", "),
            if rest.len() == 1 { "it" } else { "them" }
        )
    })
}

fn give_old_files(files: &[PathBuf], account: &str, keep: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let Ok((uid, _)) = crate::base::process::account_ids(account) else {
            return Vec::new();
        };
        let mut given: Vec<PathBuf> = Vec::new();
        for f in files {
            if uid != 0
                && !keep.contains(f)
                && !given.contains(f)
                && system::give_back(f, uid, 0, 0).unwrap_or(false)
            {
                given.push(f.clone());
            }
        }
        given
    }
    #[cfg(not(unix))]
    {
        let _ = (files, account, keep);
        Vec::new()
    }
}

fn own_logs(log: &Path) -> Vec<PathBuf> {
    std::iter::once(log.to_path_buf())
        .chain(
            (1..=crate::base::logfile::KEEP)
                .map(|n| PathBuf::from(format!("{}.{n}", log.display()))),
        )
        .collect()
}

fn start_installed(
    backend: &dyn Backend,
    manifest: &Manifest,
    arg: &str,
    again: &mut StartAgain,
    undo: Option<Undo>,
    save: impl Fn(&Manifest) -> Result<(), Failure>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    if already_running(backend, manifest, arg, out)? {
        return Ok(());
    }
    let started = backend.start(manifest);
    again.manifest = None;
    if started.is_ok() {
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    let st = backend.status(manifest);
    if let Some(undo) = undo.filter(|_| started.is_err() || !st.running) {
        let why = match started {
            Err(f) => f.message,
            Ok(()) => st.detail.clone(),
        };
        return Err(undo.put_back(backend, manifest, &why, save, out));
    }
    started?;
    if st.running {
        say(
            out,
            format!(
                "started {}{}",
                manifest.name,
                st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default()
            ),
        )?;
    } else {
        say(
            out,
            format!("warning: {} is not running: {}", manifest.name, st.detail),
        )?;
        match backend
            .journal_hint(manifest)
            .filter(|_| !manifest.log_file.exists())
        {
            Some(journal) => say(
                out,
                format!(
                    "  it wrote no log yet, so it may never have started; see the service manager's reason with: {journal}"
                ),
            )?,
            None => say(
                out,
                format!(
                    "  see why with: {}",
                    crate::base::failure::admin(&format!("service logs{arg}"))
                ),
            )?,
        }
    }
    Ok(())
}

fn already_running(
    backend: &dyn Backend,
    m: &Manifest,
    arg: &str,
    out: &mut dyn Write,
) -> Result<bool, Failure> {
    let st = backend.status(m);
    if !st.running {
        return Ok(false);
    }
    say(
        out,
        format!(
            "warning: {} was already running and was not restarted, so it still runs its previous setup{}; restart it with: {}",
            m.name,
            st.pid.map(|p| format!(" (pid {p})")).unwrap_or_default(),
            crate::base::failure::admin(&format!("service restart{arg}"))
        ),
    )?;
    Ok(true)
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
        steps.push(backend.config_folder(p.folder(), &p.config, &p.account, p.admin_writable));
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
                crate::base::failure::admin(&format!("service logs {} -n 50", new.name))
            )),
            Some(f) => Failure::new(format!(
                "the reinstalled {} did not run, and putting back its previous setup failed too: {}",
                new.name, f.message
            ))
            .hint(format!(
                "install it again from a config that works: {}",
                crate::base::failure::admin(&install_args(p, &p.config))
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
    if old.config != new.config
        && std::fs::symlink_metadata(&old.config).is_ok_and(|m| m.file_type().is_file())
    {
        let history = admin_history(&old.config);
        let history = std::fs::symlink_metadata(&history)
            .is_ok_and(|m| m.is_dir())
            .then_some(history);
        #[cfg(unix)]
        {
            let _ = system::give_plain_file(&old.config, 0, 0);
            give_history(&old.config, 0, 0);
        }
        if !is_source(&old.config, &[&old.source_config, &new.source_config]) {
            say(
                out,
                format!(
                    "note: {} holds the config the service used before, secrets included; the service no longer uses it, so delete it with: {}",
                    old.config.display(),
                    delete_hint(std::slice::from_ref(&old.config), history.as_deref())
                ),
            )?;
        }
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
    let path = crate::base::failure::path_arg(file);
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

pub fn exposed(m: &Manifest) -> bool {
    let p = Platform::current();
    m.binary == p.binary_path() && !crate::install::trust::binary_trusted(m, p)
}

pub fn exposed_note(m: &Manifest) -> String {
    format!(
        "users other than root can change {}, so burrow does not run or replace {}; move the service to its own copy with: {}",
        m.binary.parent().unwrap_or(&m.binary).display(),
        m.binary.display(),
        crate::base::failure::admin(&install_args(m, &m.config))
    )
}

pub fn recorded_version(m: &Manifest) -> String {
    if exposed(m) {
        format!("not run: {}", exposed_note(m))
    } else {
        system::binary_version(&m.binary)
    }
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

pub fn saved_env(name: &str) -> BTreeMap<String, String> {
    match (system::backend(), load_manifest(name)) {
        (Ok(b), Ok(m)) => b.read_env(&m),
        _ => BTreeMap::new(),
    }
}

pub fn resolve(name: Option<&str>) -> Result<Manifest, Failure> {
    if let Some(n) = name {
        let full = if n.starts_with("burrow") {
            n.to_string()
        } else {
            format!("burrow-{n}")
        };
        return load_manifest(&full).map_err(|f| stray::named(&full).map_or(f, |s| s.failure()));
    }
    stray::pick(all_manifests(), &stray::strays())
}

pub fn note_strays(out: &mut dyn Write) -> Result<(), Failure> {
    for s in stray::strays() {
        s.note(out)?;
    }
    Ok(())
}

fn p_log_dir() -> PathBuf {
    Platform::current().log_dir()
}

fn service_binary_for(_current: &Path, platform: Platform) -> (PathBuf, bool) {
    (platform.service_binary(), true)
}

fn place_binary(current: &Path, binary: &Path, usual: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    if let Some(dir) = binary.parent()
        && (binary != usual || !dir.exists())
    {
        system::make_dir(dir, 0o755, None)?;
    }
    #[cfg(not(unix))]
    let _ = usual;
    system::copy_file(current, binary, true)
}

fn admin_history(config: &Path) -> PathBuf {
    crate::admin::adapters::history_dir_of(config)
}

#[cfg(unix)]
fn give_history(config: &Path, uid: u32, gid: u32) {
    use rustix::fs::{FileType, Mode, OFlags};
    let folder = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    if let Ok(history) = rustix::fs::open(admin_history(config), folder, Mode::empty()) {
        let file = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        for entry in rustix::fs::Dir::read_from(&history)
            .into_iter()
            .flatten()
            .flatten()
        {
            let Ok(f) = rustix::fs::openat(&history, entry.file_name(), file, Mode::empty()) else {
                continue;
            };
            if rustix::fs::fstat(&f).is_ok_and(|st| {
                FileType::from_raw_mode(st.st_mode) == FileType::RegularFile && st.st_nlink == 1
            }) {
                let _ = std::os::unix::fs::fchown(&f, Some(uid), Some(gid));
            }
        }
        let _ = std::os::unix::fs::fchown(&history, Some(uid), Some(gid));
    }
}

fn is_source(config: &Path, sources: &[&Path]) -> bool {
    let config = crate::base::fs::canonical_or_same(config);
    sources
        .iter()
        .any(|s| crate::base::fs::canonical_or_same(s) == config)
}

fn delete_hint(files: &[PathBuf], folder: Option<&Path>) -> String {
    let quote = |p: &Path| format!("\"{}\"", p.display());
    let mut list: Vec<String> = folder.map(quote).into_iter().collect();
    list.extend(
        files
            .iter()
            .filter(|f| !folder.is_some_and(|d| f.starts_with(d)))
            .map(|f| quote(f)),
    );
    match (cfg!(windows), folder.is_some()) {
        (true, false) => format!("Remove-Item {}", list.join(", ")),
        (true, true) => format!("Remove-Item -Recurse {}", list.join(", ")),
        (false, false) => format!("sudo rm {}", list.join(" ")),
        (false, true) => format!("sudo rm -r {}", list.join(" ")),
    }
}

pub fn uninstall(name: Option<&str>, purge: bool, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    resolve(name)?;
    let operation = system::operation(out)?;
    let m = resolve(name)?;
    let _ = backend.stop(&m);
    backend.uninstall(&m)?;
    let _ = std::fs::remove_file(manifest_path(&m.name));
    say(out, format!("removed service {}", m.name))?;
    let others = all_manifests();
    let strays = stray::strays();
    let mut logs = vec![m.log_file.clone()];
    logs.extend(
        (1..=crate::base::logfile::KEEP)
            .map(|n| PathBuf::from(format!("{}.{n}", m.log_file.display()))),
    );
    let mut kept = Vec::new();
    let mut kept_folder = None;
    let real_dir = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir());
    if purge {
        if let Some(f) = &m.env_file
            && std::fs::remove_file(f).is_ok()
        {
            say(out, format!("removed {}", f.display()))?;
        }
        if real_dir(m.folder()) && std::fs::remove_dir_all(m.folder()).is_ok() {
            say(
                out,
                format!("removed {} and the files in it", m.folder().display()),
            )?;
        } else if std::fs::remove_file(&m.config).is_ok() {
            say(out, format!("removed {}", m.config.display()))?;
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
        let _ = std::fs::remove_dir(m.folder());
        kept_folder = Some(m.folder().to_path_buf()).filter(|d| real_dir(d));
    }
    let last_binary_user = m.copied_binary && !others.iter().any(|o| o.binary == m.binary);
    if let Some(note) = stray::keeps(&strays, m.binary.display()).filter(|_| last_binary_user) {
        say(out, note)?;
    } else if last_binary_user {
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
                    Some(dir) => format!(
                        "Remove-Item -Recurse {}",
                        crate::base::failure::path_arg(dir)
                    ),
                    None => format!("sudo rm {}", crate::base::failure::path_arg(&m.binary)),
                };
                say(
                    out,
                    format!(
                        "note: cannot remove {} ({}); delete it after this command ends: {later}",
                        m.binary.display(),
                        crate::base::failure::file_reason(&e)
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
    let last_account_user = m.created_account && !others.iter().any(|o| o.account == m.account);
    if let Some(note) = stray::keeps(&strays, format!("service account {}", m.account))
        .filter(|_| last_account_user)
    {
        say(out, note)?;
    } else if last_account_user {
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
            if let Some(d) = &kept_folder {
                give_history(&m.config, 0, 0);
                let _ = std::os::unix::fs::lchown(d, Some(0), Some(0));
            }
        }
        let given = give_old_files(&stray::leftovers(None), &m.account, &[]);
        if !given.is_empty() {
            say(
                out,
                format!(
                    "gave {} to root before removing service account {}, since an older install left {} owned by it",
                    given
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    m.account,
                    if given.len() == 1 { "it" } else { "them" }
                ),
            )?;
        }
        match backend.remove_account(&m.account) {
            Ok(()) => say(out, format!("removed service account {}", m.account))?,
            Err(f) => account_left = Some(f),
        }
    }
    if !kept.is_empty() || kept_folder.is_some() {
        say(
            out,
            format!(
                "kept these files for a later reinstall; delete them with:\n  {}",
                delete_hint(&kept, kept_folder.as_deref())
            ),
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

pub fn parse_version(v: &str) -> Option<semver::Version> {
    let v = v.trim().split([' ', '+']).next()?;
    semver::Version::parse(v).ok()
}

pub fn this_binary() -> Result<PathBuf, Failure> {
    let me = std::env::current_exe()
        .map_err(|e| Failure::new(format!("cannot locate this burrow binary: {e}")))?;
    Ok(crate::base::fs::canonical(&me).unwrap_or(me))
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

fn install_args(m: &Manifest, from: &Path) -> String {
    let mut args = format!(
        "service install -c {}",
        crate::base::failure::path_arg(from)
    );
    if from != m.config && service_name(from, None).ok().as_deref() != Some(m.name.as_str()) {
        args.push_str(&format!(" --name {}", m.name));
    }
    if m.admin_writable {
        args.push_str(" --admin-writable");
    }
    args
}

fn writable_args(m: &Manifest) -> String {
    install_args(
        &Manifest {
            admin_writable: true,
            ..m.clone()
        },
        &m.config,
    )
}

fn name_for(source: &Path, given: Option<&str>, installed: &[Manifest]) -> Result<String, Failure> {
    let name = service_name(source, given)?;
    let Some(owner) = installed.iter().find(|m| {
        crate::base::fs::canonical_or_same(&m.config) == crate::base::fs::canonical_or_same(source)
    }) else {
        return Ok(name);
    };
    if given.is_none() || owner.name == name {
        return Ok(owner.name.clone());
    }
    Err(Failure::usage(format!(
        "{} is the installed config of {}, so it cannot be installed as {name}",
        source.display(),
        owner.name
    ))
    .hint(format!(
        "copy it to another folder first, or change {} with: {}",
        owner.name,
        crate::base::failure::admin(&install_args(owner, &owner.config))
    )))
}

pub fn status(name: Option<&str>, out: &mut dyn Write) -> Result<u8, Failure> {
    let backend = system::backend()?;
    let m = resolve(name)?;
    let st = backend.status(&m);
    let reinstall = crate::base::failure::admin(&install_args(&m, &m.config));
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
        format!(
            "  config:  {}{}{}",
            m.config.display(),
            if m.admin_writable {
                " (the service may change it, for burrow admin)"
            } else {
                ""
            },
            missing(&m.config)
        ),
    )?;
    say(
        out,
        format!(
            "  binary:  {} ({}){}",
            m.binary.display(),
            recorded_version(&m),
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
    if let Some(note) = source_note(&m) {
        say(out, format!("  note:    {note}"))?;
    }
    Ok(u8::from(!st.running))
}

fn source_note(m: &Manifest) -> Option<String> {
    if m.source_config == m.config
        || !m.source_config.exists()
        || !differs(&m.source_config, &m.config)
    {
        return None;
    }
    Some(if m.admin_writable {
        format!(
            "{} is managed by burrow admin; {} was only its first version",
            m.config.display(),
            m.source_config.display()
        )
    } else {
        format!(
            "{} differs from the installed copy; to use it, run: {}",
            m.source_config.display(),
            crate::base::failure::admin(&install_args(m, &m.source_config))
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Kept {
    Nothing,
    Version,
    NotPlain,
}

fn keep_admin_copy(source: &Path, config: &Path) -> Result<Kept, Failure> {
    use crate::admin::core::ports::Store;
    if std::fs::symlink_metadata(config).is_err() {
        return Ok(Kept::Nothing);
    }
    let Some(installed) = system::read_plain_file(config) else {
        return Ok(Kept::NotPlain);
    };
    let (Ok(src), Ok(installed)) = (std::fs::read(source), String::from_utf8(installed)) else {
        return Ok(Kept::Nothing);
    };
    if src == installed.as_bytes() {
        return Ok(Kept::Nothing);
    }
    let store = crate::admin::adapters::FileStore::new(config);
    store.keep(&crate::admin::core::text::rev(&installed), &installed);
    match store.history_problem() {
        None => Ok(Kept::Version),
        Some(why) => Err(Failure::new(format!(
            "{} differs from the source and cannot be kept before the reinstall replaces it: {why}",
            config.display()
        ))
        .hint(format!(
            "copy {} somewhere safe, or use it as the source: {}",
            config.display(),
            crate::base::failure::admin(&format!(
                "service install -c {} --admin-writable",
                crate::base::failure::path_arg(config)
            ))
        ))),
    }
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
    let strays = stray::strays();
    if all.is_empty() && strays.is_empty() {
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
    for s in strays {
        say(out, s.line())?;
    }
    Ok(())
}

pub fn logs(name: Option<&str>, lines: usize, follow: bool) -> Result<(), Failure> {
    let m = resolve(name)?;
    let journal = system::backend().ok().and_then(|b| b.journal_hint(&m));
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
        &crate::base::failure::admin(&retry),
        journal.as_deref(),
        &mut out,
    )
}

pub fn run_service(name: &str) -> Result<u8, Failure> {
    system::run_as_service(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{in_service_folder, trustworthy, writable_hint};

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
    fn a_delete_hint_names_a_kept_folder_and_not_the_files_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("e2e-admin");
        let config = folder.join("server.yaml");
        let log = dir.path().join("e2e-admin.log");
        let hint = delete_hint(&[config.clone(), log.clone()], Some(&folder));
        let q = |p: &Path| format!("\"{}\"", p.display());
        assert!(hint.contains(&q(&folder)), "{hint}");
        assert!(hint.contains(&q(&log)), "{hint}");
        assert!(!hint.contains(&q(&config)), "{hint}");
        assert!(
            hint.contains(if cfg!(windows) { "-Recurse" } else { "rm -r" }),
            "{hint}"
        );
        let plain = delete_hint(&[log], None);
        assert!(!plain.contains("-r"), "{plain}");
    }

    #[cfg(unix)]
    #[test]
    fn a_config_the_account_swapped_for_a_link_is_never_read_into_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("server.yaml");
        let folder = dir.path().join("server");
        std::fs::create_dir(&folder).unwrap();
        let config = folder.join("server.yaml");
        let private = dir.path().join("shadow");
        std::fs::write(&source, "listen: 1\n").unwrap();
        std::fs::write(&private, "root:secret-hash:19000::::::\n").unwrap();
        std::os::unix::fs::symlink(&private, &config).unwrap();
        assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::NotPlain);
        std::fs::remove_file(&config).unwrap();
        std::fs::hard_link(&private, &config).unwrap();
        assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::NotPlain);
        let history = crate::admin::adapters::history_dir_of(&config);
        let kept: Vec<String> = std::fs::read_dir(&history)
            .map(|d| {
                d.flatten()
                    .map(|e| std::fs::read_to_string(e.path()).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default();
        assert!(!kept.iter().any(|t| t.contains("secret-hash")), "{kept:?}");
    }

    #[test]
    fn a_config_changed_by_burrow_admin_is_kept_before_a_reinstall_replaces_it() {
        use crate::admin::core::ports::Store;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("server.yaml");
        let folder = dir.path().join("server");
        std::fs::create_dir(&folder).unwrap();
        let config = folder.join("server.yaml");
        std::fs::write(&source, "listen: 1\n").unwrap();
        assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Nothing);
        std::fs::write(&config, "listen: 1\n").unwrap();
        assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Nothing);
        let changed = "listen: 1\nclients:\n  laptop: {}\n";
        std::fs::write(&config, changed).unwrap();
        assert_eq!(keep_admin_copy(&source, &config).unwrap(), Kept::Version);
        let store = crate::admin::adapters::FileStore::new(&config);
        let rev = crate::admin::core::text::rev(changed);
        assert!(store.versions().iter().any(|v| v.rev == rev));
        assert_eq!(store.version(&rev).as_deref(), Some(changed));

        let m = Manifest {
            config: config.clone(),
            source_config: source.clone(),
            admin_writable: true,
            ..record(Platform::Linux)
        };
        let note = source_note(&m).unwrap();
        assert!(note.contains("managed by burrow admin"), "{note}");
        assert!(!note.contains("install"), "{note}");
        let fixed = Manifest {
            admin_writable: false,
            ..m.clone()
        };
        let note = source_note(&fixed).unwrap();
        let from = format!(
            "to use it, run: {}",
            crate::base::failure::admin(&format!(
                "service install -c {}",
                crate::base::failure::path_arg(&source)
            ))
        );
        assert!(note.contains(&from), "{note}");
        std::fs::write(&config, "listen: 1\n").unwrap();
        assert!(source_note(&m).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_reinstall_from_the_installed_config_is_recognised_through_a_linked_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("private-etc");
        std::fs::create_dir_all(real.join("burrow/server")).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("etc")).unwrap();
        let installed = tmp.path().join("etc/burrow/server/server.yaml");
        std::fs::write(&installed, "listen: 1\n").unwrap();
        let m = Manifest {
            config: installed.clone(),
            source_config: tmp.path().join("gone/server.yaml"),
            ..record(Platform::MacOs)
        };
        let source = crate::base::fs::canonical(&installed).unwrap();
        assert_ne!(source, installed, "the folder is reached through a link");
        assert!(installs_from(&m, &source));
        assert_eq!(recorded_source(Some(&m), &source), m.source_config);
        assert_eq!(recorded_source(None, &source), source);
        let elsewhere = tmp.path().join("other.yaml");
        std::fs::write(&elsewhere, "listen: 1\n").unwrap();
        assert!(!installs_from(
            &m,
            &crate::base::fs::canonical(&elsewhere).unwrap()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_source_recorded_through_a_linked_parent_is_still_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let real = crate::base::fs::canonical(dir.path())
            .unwrap()
            .join("private-etc");
        std::fs::create_dir_all(real.join("burrow/x")).unwrap();
        std::fs::write(real.join("burrow/x.yaml"), "listen: 1\n").unwrap();
        std::fs::write(real.join("burrow/x/server.yaml"), "listen: 1\n").unwrap();
        let etc = dir.path().join("etc");
        std::os::unix::fs::symlink(&real, &etc).unwrap();
        let source = crate::base::fs::canonical(&etc.join("burrow/x.yaml")).unwrap();
        assert_ne!(source, etc.join("burrow/x.yaml"));
        assert!(is_source(&etc.join("burrow/x.yaml"), &[&source]));
        assert!(!is_source(&etc.join("burrow/x/server.yaml"), &[&source]));
    }

    #[cfg(unix)]
    #[test]
    fn a_binary_in_a_private_folder_is_copied_into_a_missing_service_folder_once() {
        system::tests::under_umask_077(
            "service::tests::a_binary_in_a_private_folder_is_copied_into_a_missing_service_folder_once",
            || {
                use std::os::unix::fs::PermissionsExt;
                let dir = tempfile::tempdir().unwrap();
                std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
                    .unwrap();
                let private = dir.path().join("root");
                std::fs::create_dir(&private).unwrap();
                let current = private.join("burrow");
                std::fs::write(&current, "the burrow binary").unwrap();
                std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o755)).unwrap();
                std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
                let usual = dir.path().join("bin/burrow");
                assert!(!usual.parent().unwrap().exists());
                place_binary(&current, &usual, &usual).unwrap();
                let folder = std::fs::metadata(usual.parent().unwrap()).unwrap();
                assert_eq!(folder.permissions().mode() & 0o777, 0o755);
                assert_eq!(std::fs::read(&usual).unwrap(), b"the burrow binary");
                assert!(std::fs::metadata(&usual).unwrap().permissions().mode() & 0o001 != 0);
                assert!(
                    system::same_content(&current, &usual),
                    "a reinstall from the same file has nothing to copy"
                );
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_trusted_binary_elsewhere_is_never_used_in_place() {
        let trusted = Path::new("/usr/bin/true");
        assert!(
            crate::install::trust::trusted_binary(trusted),
            "/usr/bin/true is root-only here"
        );
        let p = Platform::current();
        let (binary, copied) = service_binary_for(trusted, p);
        assert_eq!(binary, p.service_binary());
        assert!(copied);
        #[cfg(target_os = "linux")]
        {
            let unit = systemd::Systemd.render(&Manifest {
                binary: binary.clone(),
                ..record(Platform::Linux)
            });
            assert!(
                unit.contains(&format!("ExecStart={} ", binary.display())),
                "{unit}"
            );
            assert!(!unit.contains("/usr/bin/true"), "{unit}");
        }
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
        fn config_folder(
            &self,
            _: &Path,
            _: &Path,
            account: &str,
            writable: bool,
        ) -> Result<(), Failure> {
            self.0
                .borrow_mut()
                .push(format!("folder writable={writable} {account}"));
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
    fn the_hints_after_an_install_name_the_service_when_anything_else_is_installed() {
        assert_eq!(name_arg("burrow-home", 1, 0), "");
        assert_eq!(name_arg("burrow-home", 2, 0), " burrow-home");
        assert_eq!(name_arg("burrow-home", 1, 1), " burrow-home");
    }

    #[test]
    fn an_install_never_says_it_started_a_service_that_was_already_running() {
        let m = record(Platform::Linux);
        let rec = Recorder(Default::default(), true);
        let mut out = Vec::new();
        let mut again = StartAgain {
            backend: &rec,
            manifest: None,
        };
        start_installed(&rec, &m, "", &mut again, None, |_| Ok(()), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(
            out.starts_with("warning: burrow-server was already running and was not restarted"),
            "{out}"
        );
        assert!(out.contains("service restart"), "{out}");
        assert!(!out.contains("started burrow-server"), "{out}");
        assert!(rec.0.borrow().is_empty());
        let stopped = Recorder(Default::default(), false);
        let mut out = Vec::new();
        let mut again = StartAgain {
            backend: &stopped,
            manifest: None,
        };
        start_installed(&stopped, &m, "", &mut again, None, |_| Ok(()), &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(*stopped.0.borrow(), ["start burrow"]);
        assert!(
            out.starts_with("warning: burrow-server is not running"),
            "{out}"
        );
        assert!(!out.contains("already running"), "{out}");
    }

    #[test]
    fn an_install_records_burrows_own_account_that_an_earlier_install_left_behind() {
        let p = Platform::Linux;
        let ours = p.default_account("burrow-server");
        let (shaped, other) = (|| true, || false);
        let previous = Manifest {
            created_account: true,
            ..record(p)
        };
        let elsewhere = Manifest {
            account: "www".into(),
            ..previous.clone()
        };
        assert!(burrows_account(true, &ours, &ours, None, other));
        assert!(burrows_account(false, &ours, &ours, Some(&previous), other));
        assert!(burrows_account(false, &ours, &ours, None, shaped));
        assert!(!burrows_account(false, &ours, &ours, None, other));
        assert!(!burrows_account(false, "www", &ours, None, shaped));
        assert!(!burrows_account(
            false,
            &ours,
            &ours,
            Some(&elsewhere),
            other
        ));
    }

    #[test]
    fn an_install_refuses_a_running_unit_it_has_no_record_of_and_replaces_a_stopped_one() {
        let s = stray::Stray {
            name: "burrow-client".into(),
            label: "burrow-client.service".into(),
            config: Some(PathBuf::from("/etc/burrow/client.yaml")),
            running: true,
            pid: Some(42),
            stop: "sudo systemctl stop burrow-client".into(),
        };
        let new = Path::new("/etc/burrow/client/client.yaml");
        assert_eq!(take_over(Some(&s), new).unwrap_err(), s.failure());
        let stopped = stray::Stray {
            running: false,
            pid: None,
            ..s.clone()
        };
        let taken = take_over(Some(&stopped), new).unwrap().unwrap();
        assert_eq!(
            taken.note(&[]),
            "replaced burrow-client.service, which burrow had no record of; its old config was left as it was: /etc/burrow/client.yaml"
        );
        let old = PathBuf::from("/etc/burrow/client.yaml");
        assert_eq!(
            taken.note(std::slice::from_ref(&old)),
            "replaced burrow-client.service, which burrow had no record of; its old config was left as it was, now owned by root: /etc/burrow/client.yaml"
        );
        assert_eq!(taken.old_files(), vec![old.clone()]);
        let in_place = stray::Stray {
            config: Some(new.to_path_buf()),
            ..stopped.clone()
        };
        let taken = take_over(Some(&in_place), new).unwrap().unwrap();
        assert_eq!(
            taken.note(&[]),
            "replaced burrow-client.service, which burrow had no record of"
        );
        assert!(taken.old_files().is_empty());
        assert!(take_over(None, new).unwrap().is_none());
        let env = PathBuf::from("/etc/burrow/client.env");
        assert_eq!(given_note("burrow-client", &[old.clone(), env.clone()], Some(&old)), Some("gave /etc/burrow/client.env, left by an older install of burrow-client, to root, since the service does not use it".to_string()));
        assert_eq!(
            given_note("burrow-client", std::slice::from_ref(&old), Some(&old)),
            None
        );
        assert_eq!(given_note("burrow-client", &[], None), None);
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
            admin_writable: true,
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
                "folder writable=false old",
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
            config: p.config_dir().join("server").join("server.yaml"),
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
            admin_writable: false,
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_edit_the_installed_unit_cannot_run_is_named_with_the_way_out() {
        let secret = crate::wire::secret::Secret::generate().encode();
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
                .render_text(crate::base::style::Style { color: false });
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
    fn a_config_is_trusted_only_in_the_service_s_own_folder_whether_writable_or_not() {
        let p = Platform::current();
        let file = manifest_path("burrow-server");
        let own = p.config_dir().join("server");
        for writable in [false, true] {
            let m = Manifest {
                admin_writable: writable,
                ..record(p)
            };
            assert_eq!(m.folder(), own);
            assert!(trustworthy(&m, &file));
            let planted = [
                std::env::temp_dir().join("server").join("server.yaml"),
                p.config_dir().join("other").join("server.yaml"),
                p.config_dir().join("server.yaml"),
                own.join("deeper").join("server.yaml"),
            ];
            for config in planted {
                let m = Manifest {
                    config,
                    admin_writable: writable,
                    ..record(p)
                };
                assert!(!trustworthy(&m, &file), "{m:?}");
            }
        }
    }

    #[test]
    fn the_install_command_for_a_service_is_its_installed_config() {
        let p = Platform::current();
        let m = record(p);
        let config = crate::base::failure::path_arg(&m.config);
        assert_eq!(
            install_args(&m, &m.config),
            format!("service install -c {config}")
        );
        assert_eq!(
            writable_args(&m),
            format!("service install -c {config} --admin-writable")
        );
        let client = Manifest {
            name: "burrow-home".into(),
            role: "client".into(),
            config: p.config_dir().join("home").join("client.yaml"),
            ..record(p)
        };
        assert_eq!(
            install_args(&client, &client.config),
            format!(
                "service install -c {}",
                crate::base::failure::path_arg(&client.config)
            )
        );
        assert_eq!(
            install_args(&client, Path::new("/srv/office.yaml")),
            "service install -c /srv/office.yaml --name burrow-home"
        );
    }

    #[test]
    fn the_read_only_hint_names_the_command_for_the_installed_config() {
        let config = Platform::current()
            .config_dir()
            .join("edge")
            .join("server.yaml");
        assert!(in_service_folder(&config));
        assert!(!in_service_folder(Path::new("/srv/edge/server.yaml")));
        assert!(!in_service_folder(
            &Platform::current().config_dir().join("server.yaml")
        ));
        let hint = writable_hint(&config);
        let command = crate::base::failure::admin(&format!(
            "service install -c {} --admin-writable",
            crate::base::failure::path_arg(&config)
        ));
        assert!(hint.ends_with(&command), "{hint}");
        assert!(hint.contains("nothing moves"), "{hint}");
    }

    #[test]
    fn installing_from_an_installed_config_reinstalls_its_own_service() {
        let p = Platform::current();
        let home = Manifest {
            name: "burrow-home".into(),
            role: "client".into(),
            config: p.config_dir().join("home").join("client.yaml"),
            ..record(p)
        };
        let all = [record(p), home.clone()];
        assert_eq!(name_for(&home.config, None, &all).unwrap(), "burrow-home");
        assert_eq!(
            name_for(&home.config, Some("home"), &all).unwrap(),
            "burrow-home"
        );
        let other = name_for(&home.config, Some("office"), &all).unwrap_err();
        assert!(
            other
                .message
                .contains("is the installed config of burrow-home"),
            "{}",
            other.message
        );
        assert_eq!(
            name_for(Path::new("/srv/client.yaml"), None, &all).unwrap(),
            "burrow-client"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_admin_visitor_on_a_read_only_install_is_named() {
        let secret = |_| crate::wire::secret::Secret::generate().encode();
        let text = format!(
            "listen: 7835\nclients:\n  home: {{ secret: {} }}\nvisitors:\n  ops: {{ secret: {}, admin: write }}\nservices:\n  web: {{ client: home, bind: 8080, target: 80 }}\n",
            secret(0),
            secret(1)
        );
        let nothing = |_: &str| None;
        let loaded =
            crate::config::load_str("c.yaml", &text, crate::config::Format::Yaml, None, &nothing)
                .loaded
                .unwrap();
        let backend = systemd::Systemd;
        let m = record(Platform::Linux);
        let found = misfits_of(&backend, &m, &loaded);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "service-admin-read-only");
        assert!(!found[0].is_error());
        assert!(found[0].message.contains("visitor ops has admin access"));
        let help = found[0].help.as_deref().unwrap();
        assert!(
            help.contains("service install -c /etc/burrow/server/server.yaml --admin-writable"),
            "{help}"
        );
        let writable = Manifest {
            admin_writable: true,
            ..m
        };
        assert!(misfits_of(&backend, &writable, &loaded).is_empty());
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

    #[cfg(unix)]
    #[test]
    fn a_record_whose_bin_folder_users_can_now_change_is_managed_but_its_binary_never_runs() {
        let p = Platform::current();
        let usual = p.binary_path();
        crate::install::trust::tests::as_if_users_can_change(usual.parent().unwrap(), || {
            assert_ne!(p.service_binary(), usual);
            let file = manifest_path("burrow-server");
            let installed = Manifest {
                binary: usual.clone(),
                ..record(p)
            };
            assert!(trustworthy(&installed, &file));
            assert!(trustworthy(&record(p), &file));
            let mut forged = installed.clone();
            forged.binary = std::env::temp_dir().join("burrow");
            assert!(!trustworthy(&forged, &file));
            assert!(
                recorded_version(&installed).contains("users other than root can change"),
                "{}",
                recorded_version(&installed)
            );
            assert_eq!(service_binary_for(&usual, p).0, p.service_binary());
        });
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
    fn a_service_never_takes_the_name_of_one_of_burrow_s_own_folders() {
        let p = Platform::Windows;
        let root = p.config_dir();
        let own: Vec<PathBuf> = [
            p.state_dir(),
            p.jobs_dir(),
            p.log_dir(),
            p.state_dir().with_file_name("bin"),
            root.join("instances"),
        ]
        .into_iter()
        .filter(|d| d.parent() == Some(root.as_path()))
        .collect();
        assert_eq!(own.len(), 5);
        for dir in own {
            let short = dir.file_name().unwrap().to_string_lossy().into_owned();
            for given in [
                Path::new("/x").join(format!("{short}.yaml")),
                Path::new("/x").join(format!("{}.yaml", short.to_uppercase())),
            ] {
                let refused = service_name(&given, None).unwrap_err();
                assert!(
                    refused.message.contains("burrow's own folders")
                        && refused.message.contains("the config file name"),
                    "{short}: {}",
                    refused.message
                );
            }
            let named =
                service_name(Path::new("/x/s.yaml"), Some(&format!("burrow-{short}"))).unwrap_err();
            assert!(named.message.contains("--name"), "{}", named.message);
        }
        assert!(service_name(Path::new("/x/services-eu.yaml"), None).is_ok());
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
    fn versions_follow_semver_strictly() {
        assert_eq!(parse_version("01.2.3"), None);
        assert_eq!(parse_version("1.02.3"), None);
        assert_eq!(parse_version("1.0.0-01"), None);
        let v = |s: &str| parse_version(s).unwrap();
        assert!(v("1.0.0-1") < v("1.0.0-2"));
        assert!(v("1.0.0-1") < v("1.0.0-a"));
        assert_eq!(parse_version("1.0.0-"), None);
    }
}
