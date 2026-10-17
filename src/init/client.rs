use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::export::{CLIENT, ConfigOutput, connect_address, load_server, pick_name, secret_text};
use super::{InitOptions, Login, ask_login, resolve_output, write_new};
use crate::config::check::parse_host_port;
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::{Format, Role};
use crate::failure::Failure;
use crate::prompt::Prompter;

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
    writeln!(
        out,
        "wrote {} (allows 127.0.0.0/8; edit \"allow\" to expose other networks)",
        path.display()
    )
    .map_err(|e| Failure::new(format!("cannot write output: {e}")))?;
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

    let mut allow = Vec::new();
    let served = settings
        .services
        .iter()
        .filter(|(_, s)| s.clients.iter().any(|c| c == name));
    for (svc_name, svc) in served {
        if crate::config::check::unix::path_of(&svc.target).is_some() {
            if !allow.contains(&svc.target) {
                allow.push(svc.target.clone());
            }
            continue;
        }
        let Ok((host, first)) = parse_host_port(&svc.target) else {
            continue;
        };
        let port = crate::config::check::ports::show(first, svc.ports);
        let host_ip = host.parse::<IpAddr>().map(|ip| match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        });
        let entries: Vec<String> = match host_ip {
            Ok(IpAddr::V4(ip)) => vec![format!("{ip}/32:{port}")],
            Ok(IpAddr::V6(ip)) => vec![format!("[{ip}/128]:{port}")],
            Err(_) if host.eq_ignore_ascii_case("localhost") => {
                vec![format!("127.0.0.1/32:{port}"), format!("[::1/128]:{port}")]
            }
            Err(_) => {
                notes.push(format!(
                    "service \"{svc_name}\" targets host name {host}; add its IP address to \"allow\" on the client"
                ));
                Vec::new()
            }
        };
        for e in entries {
            if !allow.contains(&e) {
                allow.push(e);
            }
        }
    }
    if allow.is_empty() {
        notes.push("no IP targets found for this client; edit \"allow\" before starting it".into());
    }
    let mut value = client_value(server, name, &secret_text, &allow);
    if settings.insecure_plaintext {
        value["insecure_plaintext"] = Value::Bool(true);
    }
    Ok(ConfigOutput {
        text: render(&value, format),
        notes,
    })
}
