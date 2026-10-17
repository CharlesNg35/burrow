use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{InitOptions, Prompter, check_env_name, check_name, resolve_output, write_new};
use crate::config::check::parse_host_port;
use crate::config::emit::render;
use crate::config::env::EnvSource;
use crate::config::tree::{self, Value as TreeValue};
use crate::config::{Format, Loaded, Role, load_file};
use crate::failure::Failure;
use crate::secret::Secret;
use crate::transport::endpoint::Endpoint;

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
    if path.exists() && !opts.force {
        return Err(Failure::new(format!(
            "{} already exists, not overwriting it",
            path.display()
        ))
        .hint("pass --force to replace it, or -o to write somewhere else"));
    }
    let missing = |what: &str, flag: &str| {
        Failure::usage(format!("missing {what}")).hint(format!(
            "pass {flag} (stdin is not a terminal, so burrow cannot ask)"
        ))
    };
    let server = match &opts.server {
        Some(s) => s.clone(),
        None => prompt
            .ask("server host name or IP (port 7835 unless you add :PORT)")
            .ok_or_else(|| missing("server address", "--server HOST"))?,
    };
    let server = Endpoint::parse(&server)
        .map_err(|e| {
            Failure::usage(format!("invalid server address: {e}")).hint(
                "use a host name or IP, for example myserver.com, or myserver.com:2333 for another port",
            )
        })?
        .to_string();
    let name = match &opts.name {
        Some(n) => n.clone(),
        None => prompt
            .ask("client name (as in the server config)")
            .ok_or_else(|| missing("client name", "--name NAME"))?,
    };
    check_name("client name", &name)?;
    let secret_text = match (&opts.env_secret, &opts.secret) {
        (Some(var), _) => {
            check_env_name(var)?;
            format!("${{{var}}}")
        }
        (None, Some(s)) => s.clone(),
        (None, None) => prompt
            .ask("secret (from the server config)")
            .ok_or_else(|| missing("secret", "--secret VALUE or --env-secret VAR"))?,
    };
    if !secret_text.starts_with("${") {
        Secret::parse(&secret_text).map_err(|e| {
            Failure::usage(format!("the secret {}", e.describe())).hint(format!(
                "copy it from the server config, or run on the server: {} client-config NAME -o client.yaml",
                crate::failure::command()
            ))
        })?;
    }
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

pub struct ClientConfigOutput {
    pub text: String,
    pub notes: Vec<String>,
}

pub fn client_config(
    name: Option<&str>,
    server_config: &Path,
    server: Option<&str>,
    format: Format,
    env: &dyn EnvSource,
) -> Result<ClientConfigOutput, Failure> {
    let outcome = load_file(server_config, Some(Role::Server), env).map_err(|e| match &e {
        crate::config::load::LoadError::Read(io) if io.kind() == std::io::ErrorKind::NotFound => {
            Failure::usage(format!(
                "server config {} does not exist",
                server_config.display()
            ))
            .hint(format!(
                "pass the server config with -c, or create one with: {}",
                crate::failure::cmd("init server")
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
                crate::failure::command(),
                server_config.display()
            )),
        );
    };
    let names: Vec<&str> = settings.clients.keys().map(String::as_str).collect();
    let name = match (name, names.as_slice()) {
        (Some(n), _) => n,
        (None, [only]) => only,
        (None, []) => {
            return Err(
                Failure::new(format!("{} defines no clients", server_config.display())).hint(
                    format!(
                        "add one under \"clients\", with a secret from: {} gensecret",
                        crate::failure::command()
                    ),
                ),
            );
        }
        (None, _) => {
            let mut example = format!(
                "{} client-config {} -c {}",
                crate::failure::command(),
                names[0],
                server_config.display()
            );
            if let Some(s) = server {
                example.push_str(&format!(" --server {s}"));
            }
            return Err(Failure::usage(format!(
                "{} defines {} clients; which one?",
                server_config.display(),
                names.len()
            ))
            .hint(format!(
                "pass its name, for example: {example} (defined clients: {})",
                names.join(", ")
            )));
        }
    };
    let Some(_) = settings.clients.get(name) else {
        let hint = match crate::suggest::closest(name, names.iter().copied()) {
            Some(s) => format!(
                "did you mean \"{s}\"? defined clients: {}",
                names.join(", ")
            ),
            None => format!("defined clients: {}", names.join(", ")),
        };
        return Err(Failure::new(format!(
            "no client named \"{name}\" in {}",
            server_config.display()
        ))
        .hint(hint));
    };
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
                return Err(Failure::usage(
                    "client-config needs the address clients connect to",
                )
                .hint(format!(
                    "pass --server myserver.example.com, or add \"public_address: myserver.example.com\" to {shown}{port}"
                )));
            }
        },
    };
    let mut notes = Vec::new();
    let server = crate::config::address::client_address(
        &given,
        settings.listen,
        settings.websocket.as_ref().map(Into::into),
        &mut notes,
    )
    .map_err(|e| Failure::usage(format!("invalid {from}: {}", e.message)).hint(e.hint))?
    .to_string();
    let server = server.as_str();

    let raw = std::fs::read_to_string(server_config).unwrap_or_default();
    let raw_secret = tree::parse(&raw).ok().and_then(|root| {
        let node = root.get("clients")?.get(name)?.get("secret")?.clone();
        match node.value {
            TreeValue::Str(s) => Some(s),
            TreeValue::Seq(items) => items.last().and_then(|n| n.as_str().map(str::to_string)),
            _ => None,
        }
    });
    let secret_text = match raw_secret {
        Some(s) if s.contains("${") => s,
        _ => settings.clients[name]
            .secrets
            .last()
            .map(Secret::encode)
            .unwrap_or_default(),
    };

    let mut allow = Vec::new();
    for (svc_name, svc) in settings.services.iter().filter(|(_, s)| s.client == name) {
        let Ok((host, port)) = parse_host_port(&svc.target) else {
            continue;
        };
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
    Ok(ClientConfigOutput {
        text: render(&value, format),
        notes,
    })
}
