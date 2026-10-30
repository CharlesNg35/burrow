use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::export::{CLIENT, ConfigOutput, connect_address, load_server, pick_name, secret_text};
use super::{InitOptions, Login, ask_login, resolve_output, write_new};
use crate::base::failure::Failure;
use crate::base::prompt::Prompter;
use crate::config::check::parse_host_port;
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::{Format, Role};

fn allow_order(entry: &str) -> (u8, Option<IpAddr>, u32, String) {
    let (addr, port) = entry.rsplit_once(':').unwrap_or((entry, ""));
    let ip = addr
        .trim_start_matches('[')
        .split('/')
        .next()
        .and_then(|a| a.trim_end_matches(']').parse::<IpAddr>().ok());
    let first = port
        .split('-')
        .next()
        .and_then(|p| p.parse::<u32>().ok())
        .unwrap_or(0);
    (u8::from(ip.is_none()), ip, first, entry.to_string())
}

pub fn client_value(server: &str, name: &str, secret_text: &str, allow: &[String]) -> Value {
    json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "allow": allow,
    })
}

pub fn init_client(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Client, opts)?;
    let Login {
        server,
        name,
        secret_text,
    } = ask_login(Role::Client, &path, opts, prompt)?;
    let value = client_value(&server, &name, &secret_text, &["127.0.0.0/8".to_string()]);
    write_new(&path, &render(&value, format), opts.force)?;
    crate::base::output::say(
        out,
        format!(
            "wrote {} (allows 127.0.0.0/8; edit \"allow\" to expose other networks)",
            path.display()
        ),
    )?;
    Ok(path)
}

pub fn client_config(
    name: Option<&str>,
    server_config: &Path,
    server: Option<&str>,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ConfigOutput, Failure> {
    let settings = load_server(server_config, env)?;
    let names: Vec<&str> = settings.clients.keys().map(String::as_str).collect();
    let name = pick_name(CLIENT, name, &names, server_config, server)?;
    let name = name.as_str();
    let mut notes = Vec::new();
    let server = connect_address(CLIENT, &settings, server, server_config, &mut notes)?;
    let server = server.as_str();
    let secret_text = secret_text(CLIENT, server_config, name, &settings.clients[name].secrets);
    let value = client_document(&settings, name, server, &secret_text, &mut notes);
    Ok(ConfigOutput {
        text: render(&value, format),
        notes,
    })
}

pub(super) fn allow_list<'a>(
    targets: impl IntoIterator<Item = (&'a str, &'a str, u16)>,
    notes: &mut Vec<String>,
) -> Vec<String> {
    let mut allow: Vec<String> = Vec::new();
    for (service, target, count) in targets {
        let entries = if crate::config::check::unix::path_of(target).is_some() {
            vec![target.to_string()]
        } else {
            allow_entries(service, target, count, notes)
        };
        for e in entries {
            if !allow.contains(&e) {
                allow.push(e);
            }
        }
    }
    allow.sort_by_key(|e| allow_order(e));
    if allow.is_empty() {
        notes.push("no IP targets found for this client; edit \"allow\" before starting it".into());
    }
    allow
}

fn allow_entries(service: &str, target: &str, count: u16, notes: &mut Vec<String>) -> Vec<String> {
    let Ok((host, first)) = parse_host_port(target) else {
        return Vec::new();
    };
    let port = crate::config::check::ports::show(first, count);
    let host_ip = host.parse::<IpAddr>().map(|ip| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    });
    match host_ip {
        Ok(IpAddr::V4(ip)) => vec![format!("{ip}/32:{port}")],
        Ok(IpAddr::V6(ip)) => vec![format!("[{ip}/128]:{port}")],
        Err(_) if burrow_syntax::address::is_localhost(&host) => {
            vec![format!("127.0.0.1/32:{port}"), format!("[::1/128]:{port}")]
        }
        Err(_) => {
            notes.push(format!(
                "service \"{service}\" targets host name {host}; add its IP address to \"allow\" on the client"
            ));
            Vec::new()
        }
    }
}

pub fn client_document(
    settings: &crate::config::settings::ServerSettings,
    name: &str,
    server: &str,
    secret_text: &str,
    notes: &mut Vec<String>,
) -> Value {
    let served = settings
        .services
        .iter()
        .filter(|(_, s)| s.clients.iter().any(|c| c == name))
        .map(|(n, s)| (n.as_str(), s.target.as_str(), s.ports));
    let allow = allow_list(served, notes);
    let mut value = client_value(server, name, secret_text, &allow);
    if settings.insecure_plaintext {
        value["insecure_plaintext"] = Value::Bool(true);
    }
    value
}
