use burrow_admin_proto::msg::{ConnectInfo, ErrorCode};
use burrow_admin_proto::plan::{Command, Snippets};
use burrow_config::{ClientEntry, Entry, VisitorEntry};
use serde_json::Value;

use super::plan::{Computed, check_text};
use super::{ApiError, Core, Session};
use crate::config::Format;
use crate::config::emit::render;
use crate::config::settings::ServerSettings;

pub const PLACEHOLDER_SERVER: &str = "SERVER_ADDRESS";

pub fn env_var(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    out.push_str("_SECRET");
    out
}

fn address(
    settings: &ServerSettings,
    given: Option<&str>,
    section: &str,
) -> (String, bool, Vec<String>) {
    let mut notes = Vec::new();
    let wanted = given
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| settings.public_address.clone());
    match wanted {
        Some(w) => match crate::config::address::client_address(
            &w,
            settings.listen,
            settings.websocket.as_ref().map(Into::into),
            section,
            &mut notes,
        ) {
            Ok(e) => (e.to_string(), true, notes),
            Err(e) => {
                notes.push(e.message);
                (PLACEHOLDER_SERVER.to_string(), false, notes)
            }
        },
        None => (PLACEHOLDER_SERVER.to_string(), false, notes),
    }
}

fn commands(kind: &str, name: &str, server: &str, file: &str) -> Vec<Command> {
    let cmd = |os: &str, label: &str, line: String| Command {
        os: os.to_string(),
        label: label.to_string(),
        line,
    };
    let client = kind == ClientEntry::ENTITY;
    let run = if client {
        format!("burrow client -c {file}")
    } else {
        format!("burrow visit -c {file}")
    };
    let mut out = Vec::new();
    for os in ["linux", "macos", "windows"] {
        out.push(cmd(
            os,
            "Write the config (asks for the secret)",
            format!("burrow init {kind} --server {server} --name {name}"),
        ));
        out.push(cmd(os, "Run it", run.clone()));
        if client {
            let install = if os == "windows" {
                format!("burrow service install -c {file}")
            } else {
                format!("sudo burrow service install -c {file}")
            };
            let label = if os == "windows" {
                "Install as a service (administrator PowerShell)"
            } else {
                "Install as a service"
            };
            out.push(cmd(os, label, install));
        }
    }
    out
}

pub fn build(
    settings: &ServerSettings,
    kind: &str,
    name: &str,
    given_address: Option<&str>,
    secret: Option<(&str, &str)>,
) -> Result<Snippets, ApiError> {
    let client = kind == ClientEntry::ENTITY;
    let section = match kind {
        k if k == ClientEntry::ENTITY => ClientEntry::section(),
        k if k == VisitorEntry::ENTITY => VisitorEntry::section(),
        other => {
            return Err(ApiError::new(
                ErrorCode::BadRequest,
                format!("no connection details for a {other}"),
            ));
        }
    };
    let known = if client {
        settings.clients.contains_key(name)
    } else {
        settings.visitors.contains_key(name)
    };
    if !known {
        return Err(ApiError::new(
            ErrorCode::BadRequest,
            format!("there is no {kind} named \"{name}\""),
        ));
    }
    let (server, server_known, mut notes) = address(settings, given_address, section);
    let env = env_var(name);
    let placeholder = format!("${{{env}}}");
    let secret_text = secret
        .map(|(v, _)| v.to_string())
        .unwrap_or(placeholder.clone());
    let document = |secret_text: &str, notes: &mut Vec<String>| -> Value {
        if client {
            crate::init::client_document(settings, name, &server, secret_text, notes)
        } else {
            crate::init::visitor_document(settings, name, &server, secret_text, notes)
        }
    };
    let config = render(&document(&secret_text, &mut notes), Format::Yaml);
    let env_config = render(&document(&placeholder, &mut Vec::new()), Format::Yaml);
    let file_name = format!("{kind}.yaml");
    Ok(Snippets {
        kind: kind.to_string(),
        name: name.to_string(),
        server: server.clone(),
        server_known,
        file_name: file_name.clone(),
        config,
        with_secret: secret.is_some(),
        secret_fp: secret.map(|(_, fp)| fp.to_string()),
        env_var: env,
        env_config,
        commands: commands(kind, name, &server, &file_name),
    })
}

pub fn connect_info(core: &Core, _s: &Session, req: &ConnectInfo) -> Result<Snippets, ApiError> {
    let text = core.read()?;
    let checked = check_text(core, &text);
    let Some(settings) = checked.settings else {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "the config on the server has errors; fix them first",
        ));
    };
    build(
        &settings,
        &req.kind,
        &req.name,
        req.address.as_deref(),
        None,
    )
}

pub fn after_apply(core: &Core, c: &Computed) -> Vec<Snippets> {
    let checked = check_text(core, &c.new_text);
    let Some(settings) = checked.settings else {
        return Vec::new();
    };
    c.fresh
        .iter()
        .filter_map(|f| {
            let kind = match f.path.first() {
                Some(s) if s == ClientEntry::section() => ClientEntry::ENTITY,
                Some(s) if s == VisitorEntry::section() => VisitorEntry::ENTITY,
                _ => return None,
            };
            let name = f.path.get(1)?;
            build(&settings, kind, name, None, Some((&f.value, &f.fp))).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_are_shell_safe() {
        assert_eq!(env_var("home"), "HOME_SECRET");
        assert_eq!(env_var("lab-2"), "LAB_2_SECRET");
    }
}
