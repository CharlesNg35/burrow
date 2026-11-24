use burrow_admin_proto::msg::{ConnectInfo, ErrorCode};
use burrow_admin_proto::plan::{Command, Snippets};
use burrow_config::{ClientEntry, Entry, VisitorEntry};
use serde_json::Value;

use super::plan::{Computed, check_docs};
use super::{ApiError, Core};
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
    let wanted = settings.public_address.clone().or_else(|| {
        given
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    match wanted {
        Some(w) => match burrow_syntax::address::client_address(
            &w,
            settings.listen,
            settings
                .websocket
                .as_ref()
                .map(crate::config::settings::WebSocketSettings::shape),
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
    let mut out = Vec::new();
    for os in ["linux", "macos", "windows"] {
        let bin = if os == "windows" {
            r".\burrow.exe"
        } else {
            "./burrow"
        };
        out.push(cmd(
            os,
            "Write the config (asks for the secret)",
            format!("{bin} init {kind} --server {server} --name {name}"),
        ));
        let run = if client {
            format!("{bin} client -c {file}")
        } else {
            format!("{bin} visit -c {file}")
        };
        out.push(cmd(os, "Run it", run));
        if client {
            let install = if os == "windows" {
                format!("{bin} service install -c {file}")
            } else {
                format!("sudo {bin} service install -c {file}")
            };
            let label = if os == "windows" {
                "Or install it as a service in an administrator PowerShell, to keep it running in the background"
            } else {
                "Or install it as a service, to keep it running in the background"
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
        downloads: None,
    })
}

pub fn connect_info(core: &Core, req: &ConnectInfo) -> Result<Snippets, ApiError> {
    let docs = core.docs()?;
    let checked = check_docs(core, &docs);
    let Some(settings) = checked.settings else {
        return Err(ApiError::new(
            ErrorCode::Invalid,
            "the config on the server has errors; fix them first",
        ));
    };
    let mut sn = build(
        &settings,
        &req.kind,
        &req.name,
        req.address.as_deref(),
        None,
    )?;
    sn.downloads = super::downloads::current(core.live.releases());
    Ok(sn)
}

pub fn after_apply(core: &Core, c: &Computed, address: Option<&str>) -> Vec<Snippets> {
    let checked = check_docs(core, &c.new_docs);
    let Some(settings) = checked.settings else {
        return Vec::new();
    };
    let downloads = super::downloads::current(core.live.releases());
    c.fresh
        .iter()
        .filter_map(|f| {
            let kind = match f.path.first() {
                Some(s) if s == ClientEntry::section() => ClientEntry::ENTITY,
                Some(s) if s == VisitorEntry::section() => VisitorEntry::ENTITY,
                _ => return None,
            };
            let name = f.path.get(1)?;
            let mut sn = build(&settings, kind, name, address, Some((&f.value, &f.fp))).ok()?;
            sn.downloads = downloads.clone();
            Some(sn)
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

    #[test]
    fn commands_cover_three_systems_and_a_client_service() {
        let c = commands("client", "lab", "tunnel.example.com:7835", "client.yaml");
        let systems: std::collections::BTreeSet<&str> = c.iter().map(|x| x.os.as_str()).collect();
        assert_eq!(
            systems.into_iter().collect::<Vec<_>>(),
            ["linux", "macos", "windows"]
        );
        let service = |os: &str| {
            c.iter()
                .find(|x| x.os == os && x.line.contains("service install"))
                .map(|x| x.label.clone())
        };
        assert_eq!(
            service("linux").as_deref(),
            Some("Or install it as a service, to keep it running in the background")
        );
        assert!(service("windows").is_some_and(|l| l.contains("administrator PowerShell")));
        let v = commands("visitor", "lab", "tunnel.example.com:7835", "visitor.yaml");
        assert!(v.iter().all(|x| !x.line.contains("service install")));
    }
}
