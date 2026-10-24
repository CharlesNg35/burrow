use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::checks::{check_config, needs_low_ports};
use super::previous::{
    StartAgain, Takeover, Undo, give_old_files, given_note, own_logs, release_previous,
    start_installed, take_over,
};
use super::uninstall::{Kept, keep_admin_copy};
use super::{Backend, background, name_for, ports, save_manifest, say, stray, system};
use crate::base::failure::Failure;
use crate::config::Role;
use crate::config::env::referenced_env;
use crate::install::{Manifest, Platform, all_manifests, load_manifest};

pub struct InstallOptions {
    pub config: PathBuf,
    pub name: Option<String>,
    pub run_as: Option<String>,
    pub start: bool,
    pub print: bool,
    pub worker: Option<PathBuf>,
    pub admin_writable: bool,
}

pub(super) fn recorded_source(existing: Option<&Manifest>, source: &Path) -> PathBuf {
    match existing {
        Some(m) if crate::base::fs::canonical_or_same(&m.config) == source => {
            m.source_config.clone()
        }
        _ => source.to_path_buf(),
    }
}

pub(super) fn installs_from(existing: &Manifest, source: &Path) -> bool {
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
            backend.status(&m).pid.map(|pid| ports::PortUser {
                pid,
                name: m.name,
                managed: true,
                stop: None,
            })
        });
        let strays = stray::strays().into_iter().filter_map(|s| {
            s.pid.map(|pid| ports::PortUser {
                pid,
                name: s.label,
                managed: false,
                stop: Some(s.stop),
            })
        });
        managed.chain(strays).collect()
    };
    for (proto, addr, label) in &checked.listens {
        ports::check_free(*proto, *addr, label, &users)?;
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

pub(super) fn name_arg(name: &str, managed: usize, strays: usize) -> String {
    if managed + strays <= 1 {
        String::new()
    } else {
        format!(" {name}")
    }
}

pub(super) fn burrows_account(
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

pub(super) fn place_binary(current: &Path, binary: &Path, usual: &Path) -> Result<(), Failure> {
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

pub(super) fn service_binary_for(_current: &Path, platform: Platform) -> (PathBuf, bool) {
    (platform.service_binary(), true)
}
