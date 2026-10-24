use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::{Backend, install_args, system, writable_args};
use crate::base::failure::Failure;
use crate::base::style::Style;
use crate::config::env::referenced_env;
use crate::config::{Loaded, Role, load_file};
use crate::install::{Manifest, load_manifest};
use crate::instance::Identity;
use burrow_config::Protocol;

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

pub(super) fn needs_low_ports(backend: &dyn Backend, addr: &SocketAddr) -> bool {
    addr.port() < 1024 && !(backend.wildcard_low_ports() && addr.ip().is_unspecified())
}

pub(super) fn misfits_of(
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

pub(super) struct Checked {
    pub(super) role: Role,
    pub(super) identity: Option<Identity>,
    pub(super) listens: Vec<(Protocol, SocketAddr, String)>,
    pub(super) files: Vec<(String, PathBuf)>,
    pub(super) admins: Vec<String>,
}

pub(super) fn check_config(
    path: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Checked, Failure> {
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
