use std::io::Write;

use super::uninstall::{differs, missing};
use super::{background, install_args, logview, recorded_version, resolve, say, stray, system};
use crate::base::failure::Failure;
use crate::install::{Manifest, all_manifests};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Reload,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Reload => "reload",
        }
    }
}

pub(super) fn interrupts(action: Action) -> bool {
    action == Action::Restart || (action == Action::Reload && cfg!(windows))
}

pub fn control(action: Action, name: Option<&str>, out: &mut dyn Write) -> Result<(), Failure> {
    system::backend()?;
    system::require_admin()?;
    let m = resolve(name)?;
    if !interrupts(action) {
        return control_now(action, Some(&m.name), out);
    }
    background::Run::new(action.as_str())?.start(
        &format!("restarting {}", m.name),
        vec![
            "service".into(),
            action.as_str().into(),
            m.name.clone().into(),
        ],
        None,
        out,
    )
}

pub fn control_now(action: Action, name: Option<&str>, out: &mut dyn Write) -> Result<(), Failure> {
    let backend = system::backend()?;
    system::require_admin()?;
    let m = resolve(name)?;
    match action {
        Action::Start => backend.start(&m)?,
        Action::Stop => backend.stop(&m)?,
        Action::Restart => {
            let _ = backend.stop(&m);
            backend.start(&m)?;
        }
        Action::Reload => backend.reload(&m)?,
    }
    let mut st = backend.status(&m);
    for _ in 0..15 {
        if st.running || action == Action::Stop {
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

pub(super) fn source_note(m: &Manifest) -> Option<String> {
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
