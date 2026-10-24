use std::path::Path;

use crate::base::failure::Failure;
use crate::config::env::EnvSource;
use crate::config::settings::ServerSettings;
use crate::config::tree::{self, Value as TreeValue};
use crate::config::{Loaded, Role, load_file};
use crate::wire::secret::Secret;

#[derive(Clone, Copy)]
pub(super) struct Kind {
    pub word: &'static str,
    pub section: &'static str,
}

pub(super) const CLIENT: Kind = Kind {
    word: "client",
    section: "clients",
};

pub(super) const VISITOR: Kind = Kind {
    word: "visitor",
    section: "visitors",
};

#[derive(Debug)]
pub struct ConfigOutput {
    pub text: String,
    pub notes: Vec<String>,
}

pub(super) fn load_server(
    server_config: &Path,
    env: &dyn EnvSource,
) -> Result<Box<ServerSettings>, Failure> {
    let outcome = load_file(server_config, Some(Role::Server), env).map_err(|e| match &e {
        crate::config::load::LoadError::Read(io) if io.kind() == std::io::ErrorKind::NotFound => {
            Failure::usage(format!(
                "server config {} does not exist",
                server_config.display()
            ))
            .hint(format!(
                "pass the server config with -c, or create one with: {}",
                crate::base::failure::cmd("init server")
            ))
        }
        _ => {
            let (m, h) = e.describe(server_config);
            Failure::new(m).hint(h)
        }
    })?;
    let Some(Loaded::Server(settings, _)) = outcome.loaded else {
        return Err(
            Failure::new(format!("{} has errors", server_config.display())).hint(format!(
                "see them with: {} validate -c {}",
                crate::base::failure::command(),
                server_config.display()
            )),
        );
    };
    Ok(settings)
}

pub(super) fn pick_name(
    kind: Kind,
    given: Option<&str>,
    names: &[&str],
    server_config: &Path,
    server: Option<&str>,
) -> Result<String, Failure> {
    let Kind { word, section } = kind;
    let name = match (given, names) {
        (Some(n), _) => n,
        (None, [only]) => only,
        (None, []) => {
            return Err(
                Failure::new(format!("{} defines no {section}", server_config.display())).hint(
                    format!(
                        "add one under \"{section}\", with a secret from: {} gensecret",
                        crate::base::failure::command()
                    ),
                ),
            );
        }
        (None, _) => {
            let mut example = format!(
                "{} {word}-config {} -c {}",
                crate::base::failure::command(),
                names[0],
                server_config.display()
            );
            if let Some(s) = server {
                example.push_str(&format!(" --server {s}"));
            }
            return Err(Failure::usage(format!(
                "{} defines {} {section}; which one?",
                server_config.display(),
                names.len()
            ))
            .hint(format!(
                "pass its name, for example: {example} (defined {section}: {})",
                names.join(", ")
            )));
        }
    };
    if !names.contains(&name) {
        let defined = if names.is_empty() {
            format!("no {section} are defined")
        } else {
            format!("defined {section}: {}", names.join(", "))
        };
        let hint = match crate::base::suggest::closest(name, names.iter().copied()) {
            Some(s) => format!("did you mean \"{s}\"? {defined}"),
            None => defined,
        };
        return Err(Failure::new(format!(
            "no {word} named \"{name}\" in {}",
            server_config.display()
        ))
        .hint(hint));
    }
    Ok(name.to_string())
}

pub(super) fn connect_address(
    kind: Kind,
    settings: &ServerSettings,
    server: Option<&str>,
    server_config: &Path,
    notes: &mut Vec<String>,
) -> Result<String, Failure> {
    let shown = server_config.display();
    let (given, from) = match server.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => (s.to_string(), "--server"),
        None => match &settings.public_address {
            Some(p) => (p.clone(), "public_address"),
            None => {
                let port = settings
                    .listen
                    .map(|l| format!(" (the port, {}, comes from listen)", l.port()))
                    .unwrap_or_default();
                return Err(Failure::usage(format!(
                    "{}-config needs the address {} connect to",
                    kind.word, kind.section
                ))
                .hint(format!(
                    "pass --server myserver.example.com, or add \"public_address: myserver.example.com\" to {shown}{port}"
                )));
            }
        },
    };
    burrow_syntax::address::client_address(
        &given,
        settings.listen,
        settings
            .websocket
            .as_ref()
            .map(crate::config::settings::WebSocketSettings::shape),
        kind.section,
        notes,
    )
    .map(|e| e.to_string())
    .map_err(|e| Failure::usage(format!("invalid {from}: {}", e.message)).hint(e.hint))
}

pub(super) fn secret_text(
    kind: Kind,
    server_config: &Path,
    name: &str,
    secrets: &[Secret],
) -> String {
    let raw = std::fs::read_to_string(server_config).unwrap_or_default();
    let raw_secret = tree::parse(&raw).ok().and_then(|root| {
        let node = root.get(kind.section)?.get(name)?.get("secret")?.clone();
        match node.value {
            TreeValue::Str(s) => Some(s),
            TreeValue::Seq(items) => items.last().and_then(|n| n.as_str().map(str::to_string)),
            _ => None,
        }
    });
    match raw_secret {
        Some(s) if s.contains("${") => s,
        _ => secrets.last().map(Secret::encode).unwrap_or_default(),
    }
}
