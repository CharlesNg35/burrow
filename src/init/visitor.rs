use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::export::{ConfigOutput, VISITOR, connect_address, load_server, pick_name, secret_text};
use super::{InitOptions, Login, Prompter, ask_login, resolve_output, write_new};
use crate::config::check::parse_host_port;
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::model::Protocol;
use crate::config::{Format, Role};
use crate::failure::Failure;

pub fn init_visitor(
    opts: &InitOptions,
    prompt: &mut dyn Prompter,
    out: &mut dyn Write,
) -> Result<PathBuf, Failure> {
    let (path, format) = resolve_output(Role::Visitor, opts)?;
    let Login {
        server,
        name,
        secret_text,
    } = ask_login(Role::Visitor, &path, opts, prompt)?;
    let value = json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "forwards": {},
    });
    write_new(&path, &render(&value, format), opts.force)?;
    writeln!(
        out,
        "wrote {}; add the services to forward, for example:\n\n  forwards:\n    db: {{ service: db, local: 5432 }}\n\nor print a ready config on the server with: {} visitor-config {name} -o {}",
        path.display(),
        crate::failure::command(),
        path.file_name().map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into_owned())
    )
    .map_err(|e| Failure::new(format!("cannot write output: {e}")))?;
    Ok(path)
}

pub fn visitor_config(
    name: Option<&str>,
    server_config: &Path,
    server: Option<&str>,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ConfigOutput, Failure> {
    let settings = load_server(server_config, env)?;
    let names: Vec<&str> = settings.visitors.keys().map(String::as_str).collect();
    let name = pick_name(VISITOR, name, &names, server_config, server)?;
    let auth = &settings.visitors[&name];
    let services: Vec<(&String, u16, u16)> = settings
        .services
        .iter()
        .filter(|(_, s)| s.protocol == Protocol::Tcp && s.visitors.contains(&name))
        .map(|(n, s)| {
            let port = parse_host_port(&s.target).map_or(0, |(_, p)| p);
            (n, port, s.ports.max(1))
        })
        .collect();
    if services.is_empty() && auth.reach.is_empty() {
        return Err(Failure::new(format!(
            "visitor \"{name}\" cannot reach anything in {}",
            server_config.display()
        ))
        .hint(format!(
            "add it to a service, for example visitors: [{name}], or give it reach: {{ CLIENT: any }}"
        )));
    }
    let mut notes = Vec::new();
    let server = connect_address(VISITOR, &settings, server, server_config, &mut notes)?;
    let secret_text = secret_text(VISITOR, server_config, &name, &auth.secrets);

    let mut used: Vec<u16> = Vec::new();
    let mut free = |first: u16, count: u16| {
        let mut local = first;
        let taken =
            |at: u16, used: &[u16]| (0..count).any(|i| used.contains(&at.saturating_add(i)));
        while taken(local, &used) || u32::from(local) + u32::from(count) > 65536 {
            local = if u32::from(local) + u32::from(count) >= 65535 {
                10000
            } else {
                local + 1
            };
        }
        used.extend((0..count).map(|i| local + i));
        local
    };
    let mut forwards = Map::new();
    for (service, port, count) in services {
        let local = free(
            match port {
                0 => 10000,
                p if p < 1024 => p + 10000,
                p => p,
            },
            count,
        );
        let local = if count > 1 {
            Value::String(crate::config::check::ports::show(local, count))
        } else {
            json!(local)
        };
        forwards.insert(
            service.clone(),
            json!({ "service": service, "local": local }),
        );
    }
    for client in &auth.reach {
        let local = free(1080, 1);
        let key = if forwards.contains_key(client.as_str()) {
            format!("{client}-socks")
        } else {
            client.clone()
        };
        forwards.insert(
            key,
            json!({ "client": client, "socks": true, "local": local }),
        );
        notes.push(format!(
            "127.0.0.1:{local} is a SOCKS5 proxy into client \"{client}\": point an app at it, or forward one target with: {} visit --client {client} --target 127.0.0.1:22 --local 2222",
            crate::failure::command()
        ));
    }
    let value = json!({
        "server": server,
        "name": name,
        "secret": secret_text,
        "forwards": Value::Object(forwards),
    });
    Ok(ConfigOutput {
        text: render(&value, format),
        notes,
    })
}
