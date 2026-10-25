use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::checks::{
    Checked, check_config, check_files_readable, check_ports_free, refuse_another_source,
    refuse_running_outside, refuse_unfit,
};
use super::previous::{
    StartAgain, Takeover, Undo, give_old_files, given_note, own_logs, release_previous,
    start_installed, take_over,
};
use super::uninstall::{Kept, keep_admin_copy};
use super::{Backend, background, name_for, save_manifest, say, stray, system};
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
    let source = readable_source(opts)?;
    let name = name_for(&source, opts.name.as_deref(), &all_manifests())?;
    let src_text = std::fs::read_to_string(&source).unwrap_or_default();
    let env = env_for(opts, &src_text);
    let checked = check_config(&source, &env)?;
    refuse_another_source(&name, &source)?;
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
        web_state: (checked.letsencrypt && checked.role == Role::Server)
            .then(|| crate::install::service_state_dir(&short)),
    };
    refuse_unfit(&*backend, &checked, &manifest)?;

    if opts.print {
        return print_unit(&*backend, &name, &manifest, out);
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
        return reinstall_in_background(opts, &source, &name, &env, out);
    }
    let _operation = operation;
    let mut again = StartAgain {
        backend: &*backend,
        manifest: previous.as_ref().filter(|_| was_running),
    };
    if let Some(p) = again.manifest {
        backend.stop(p)?;
    }

    refuse_running_outside(&checked)?;
    check_ports_free(&*backend, &checked)?;

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
    check_files_readable(&*backend, &checked, &account)?;

    backend.prepare_dirs(&account)?;
    super::uninstall::say_skipped(&backend.state_folder(&manifest)?, &account, out)?;
    let kept_binary = keep_previous_binary(again.manifest, &binary, copy_binary, &current);
    let _leftover = Leftover(kept_binary.clone());
    if copy_binary && !system::same_content(&current, &binary) {
        copy_service_binary(&current, &binary, platform, &name, out)?;
    }
    let undo = again.manifest.map(|p| Undo {
        manifest: p.clone(),
        config: system::read_plain_file(&p.config),
        env: backend.read_env(p),
        binary: kept_binary.clone().map(|kept| (kept, binary.clone())),
    });
    backend.config_folder(&folder, &config, &account, writable)?;
    if source != crate::base::fs::canonical_or_same(&config) {
        copy_config(&source, &config, writable, out)?;
    }
    backend.secure_file(&config, &account)?;
    backend.config_folder(&folder, &config, &account, writable)?;
    if writable {
        say_writable(&checked, &folder, out)?;
    }
    if let Some(f) = &env_file {
        save_env(&*backend, &manifest, &env, f, out)?;
    }
    backend.install(&manifest)?;
    if !opts.start {
        again.manifest = None;
    }
    save_manifest(&manifest)?;
    fresh.account = None;
    say_installed(
        &*backend,
        &manifest,
        &config,
        &old_files,
        taken.as_ref(),
        out,
    )?;
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
    say_next_steps(&config, &arg, out)
}

fn readable_source(opts: &InstallOptions) -> Result<PathBuf, Failure> {
    crate::base::fs::canonical(&opts.config).map_err(|e| {
        Failure::usage(format!(
            "cannot read {}: {}",
            opts.config.display(),
            crate::base::failure::file_reason(&e)
        ))
    })
}

fn print_unit(
    backend: &dyn Backend,
    name: &str,
    manifest: &Manifest,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    say(
        out,
        format!(
            "# {} service {name}, as installed by: burrow service install",
            backend.kind()
        ),
    )?;
    say(out, backend.render(manifest))?;
    Ok(())
}

fn keep_previous_binary(
    previous: Option<&Manifest>,
    binary: &Path,
    copy_binary: bool,
    current: &Path,
) -> Option<PathBuf> {
    (previous.is_some_and(|p| p.binary == binary)
        && copy_binary
        && binary.exists()
        && !system::same_content(current, binary))
    .then(|| binary.with_extension("reinstall"))
    .filter(|kept| system::copy_file(binary, kept, true).is_ok())
}

fn save_env(
    backend: &dyn Backend,
    manifest: &Manifest,
    env: &BTreeMap<String, String>,
    file: &Path,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    backend.write_env(manifest, env)?;
    say(
        out,
        format!(
            "saved {} for the service in {} (readable by the service only)",
            env.keys().cloned().collect::<Vec<_>>().join(", "),
            file.display()
        ),
    )
}

fn say_installed(
    backend: &dyn Backend,
    manifest: &Manifest,
    config: &Path,
    old_files: &[PathBuf],
    taken: Option<&Takeover>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let name = &manifest.name;
    let account = &manifest.account;
    say(
        out,
        format!(
            "installed {} service {name} ({}, running as {account}, starts at boot, restarts on failure)",
            backend.kind(),
            manifest.role
        ),
    )?;
    let mut own = own_logs(&manifest.log_file);
    own.push(config.to_path_buf());
    own.extend(manifest.env_file.clone());
    let given = give_old_files(old_files, account, &own);
    if let Some(t) = taken {
        say(out, format!("note: {}", t.note(&given)))?;
    }
    if let Some(note) = given_note(name, &given, taken.and_then(|t| t.old_config.as_ref())) {
        say(out, format!("note: {note}"))?;
    }
    Ok(())
}

fn env_for(opts: &InstallOptions, src_text: &str) -> BTreeMap<String, String> {
    match &opts.worker {
        Some(log) => background::take_secrets(log),
        None => referenced_env(src_text)
            .into_iter()
            .filter_map(|k| std::env::var(&k).ok().map(|v| (k, v)))
            .collect(),
    }
}

fn reinstall_in_background(
    opts: &InstallOptions,
    source: &Path,
    name: &str,
    env: &BTreeMap<String, String>,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "install".into(),
        "--config".into(),
        source.to_path_buf().into(),
        "--name".into(),
        name.to_string().into(),
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
    background::Run::new("install")?.start(&format!("reinstalling {name}"), args, Some(env), out)
}

fn copy_service_binary(
    current: &Path,
    binary: &Path,
    platform: Platform,
    name: &str,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    place_binary(current, binary, &platform.binary_path())?;
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
    Ok(())
}

fn copy_config(
    source: &Path,
    config: &Path,
    writable: bool,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    if writable {
        match keep_admin_copy(source, config)? {
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
    system::copy_file(source, config, false)?;
    say(
        out,
        format!("copied {} -> {}", source.display(), config.display()),
    )
}

fn say_writable(checked: &Checked, folder: &Path, out: &mut dyn Write) -> Result<(), Failure> {
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
    )
}

fn say_next_steps(config: &Path, arg: &str, out: &mut dyn Write) -> Result<(), Failure> {
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
